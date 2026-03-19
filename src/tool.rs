use std::io::Write as _;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

use git_surgeon::diff::DiffHunk;

const PATCH_ENV_VAR: &str = "JJ_HUNK_TOOL_PATCH";

/// A hunk spec: (hunk_id, hunk, line_ranges).
pub type HunkSpec<'a> = (&'a str, &'a DiffHunk, Vec<(usize, usize)>);

/// Build a combined patch from selected hunks with optional line ranges.
pub fn build_combined_patch(specs: &[HunkSpec<'_>], reverse: bool) -> Result<String> {
    let mut combined = String::new();
    for (id, hunk, ranges) in specs {
        git_surgeon::diff::check_supported(hunk, id)?;
        let patched = if !ranges.is_empty() {
            git_surgeon::patch::slice_hunk_multi(hunk, ranges, reverse)?
        } else if reverse {
            git_surgeon::patch::slice_hunk(hunk, 1, hunk.lines.len(), true)?
        } else {
            (*hunk).clone()
        };
        combined.push_str(&git_surgeon::patch::build_patch(&patched));
    }
    Ok(combined)
}

/// Write a temp jj config TOML that defines jj-hunk-tool as a merge tool.
fn write_tool_config(exe: &Path) -> Result<tempfile::NamedTempFile> {
    let mut config_file = tempfile::Builder::new()
        .suffix(".toml")
        .tempfile()
        .context("creating temp config file")?;
    write!(
        config_file,
        "[merge-tools.jj-hunk-tool]\nprogram = {exe:?}\nedit-args = [\"_jj-tool\", \"$left\", \"$right\"]\n",
        exe = exe.display().to_string(),
    )
    .context("writing config")?;
    Ok(config_file)
}

/// Run a jj command with our tool configured.
fn run_jj_with_tool(jj_args: &[&str], patch_content: &str) -> Result<()> {
    let exe = std::env::current_exe().context("finding own executable")?;

    let mut patch_file = tempfile::NamedTempFile::new().context("creating temp patch file")?;
    patch_file
        .write_all(patch_content.as_bytes())
        .context("writing patch")?;

    let config_file = write_tool_config(&exe)?;

    let mut cmd = Command::new("jj");
    cmd.args(jj_args);
    cmd.args(["--config-file", &config_file.path().display().to_string()]);
    cmd.args(["--tool", "jj-hunk-tool"]);
    cmd.env(PATCH_ENV_VAR, patch_file.path());
    // Prevent jj from ever opening an interactive editor
    cmd.env("EDITOR", "true");

    let output = cmd.output().context("running jj")?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.is_empty() {
        eprint!("{stderr}");
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.is_empty() {
        eprint!("{stdout}");
    }

    if !output.status.success() {
        bail!("jj command failed");
    }

    Ok(())
}

/// Commit selected hunks by writing a patch and invoking jj commit --tool.
pub fn commit_hunks(
    specs: &[HunkSpec<'_>],
    revision: &Option<String>,
    message: Option<&str>,
) -> Result<()> {
    let patch_content = build_combined_patch(specs, false)?;
    if patch_content.is_empty() {
        bail!("no hunks selected");
    }

    let mut args: Vec<&str> = Vec::new();
    if revision.is_some() {
        args.push("split");
        if let Some(rev) = revision {
            args.extend_from_slice(&["-r", rev]);
        }
    } else {
        args.push("commit");
    }
    let msg_storage;
    if let Some(msg) = message {
        msg_storage = msg.to_string();
        args.extend_from_slice(&["-m", &msg_storage]);
    }

    run_jj_with_tool(&args, &patch_content)?;
    Ok(())
}

/// Discard selected hunks by reverse-applying the patch directly.
pub fn discard_hunks(specs: &[HunkSpec<'_>], _revision: &Option<String>) -> Result<()> {
    let patch_content = build_combined_patch(specs, false)?;
    if patch_content.is_empty() {
        bail!("no hunks selected");
    }

    // Apply in reverse directly to the working copy
    let mut child = Command::new("patch")
        .args(["-p1", "--silent", "--reverse"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .context("running patch")?;

    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(patch_content.as_bytes())
        .context("writing to patch stdin")?;

    let status = child.wait().context("waiting for patch")?;
    if !status.success() {
        bail!("patch --reverse failed (exit code: {:?})", status.code());
    }

    Ok(())
}

/// Rewrite a revision in-place, keeping only the selected hunks.
pub fn diffedit_hunks(specs: &[HunkSpec<'_>], revision: &str) -> Result<()> {
    let patch_content = build_combined_patch(specs, false)?;
    if patch_content.is_empty() {
        bail!("no hunks selected");
    }
    run_jj_with_tool(&["diffedit", "-r", revision], &patch_content)
}

/// Restore selected hunks from one revision into another.
pub fn restore_hunks(specs: &[HunkSpec<'_>], from: &str, to: Option<&str>) -> Result<()> {
    let patch_content = build_combined_patch(specs, false)?;
    if patch_content.is_empty() {
        bail!("no hunks selected");
    }
    let mut args = vec!["restore", "--changes-in", from];
    if let Some(target) = to {
        args.extend_from_slice(&["--to", target]);
    }
    run_jj_with_tool(&args, &patch_content)
}

/// JJ tool protocol handler.
///
/// JJ invokes: `jj-hunk-tool _jj-tool $left $right`
/// - `$left` = parent/base state directory (read-only)
/// - `$right` = current state directory (writable)
///
/// Algorithm:
/// 1. Read patch path from JJ_HUNK_TOOL_PATCH env var
/// 2. Reset $right to match $left (copy all files from left, remove extras)
/// 3. Apply the patch to $right
pub fn jj_tool_apply(left: &str, right: &str) -> Result<()> {
    let patch_path = std::env::var(PATCH_ENV_VAR)
        .with_context(|| format!("{PATCH_ENV_VAR} environment variable not set"))?;

    let left_path = Path::new(left);
    let right_path = Path::new(right);

    // Step 1: Reset $right to $left state
    reset_dir_to(left_path, right_path)?;

    // Step 2: Apply the pre-computed patch
    let status = Command::new("patch")
        .args(["-p1", "--silent", "-i"])
        .arg(&patch_path)
        .current_dir(right_path)
        .status()
        .context("failed to run patch")?;

    if !status.success() {
        bail!("patch failed to apply (exit code: {:?})", status.code());
    }

    Ok(())
}

/// Reset `dst` directory to match `src` directory contents.
fn reset_dir_to(src: &Path, dst: &Path) -> Result<()> {
    remove_dir_contents(dst)?;
    copy_dir_recursive(src, dst)?;
    Ok(())
}

fn remove_dir_contents(dir: &Path) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading dir {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            std::fs::remove_dir_all(&path)
                .with_context(|| format!("removing dir {}", path.display()))?;
        } else {
            std::fs::remove_file(&path)
                .with_context(|| format!("removing file {}", path.display()))?;
        }
    }
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    for entry in std::fs::read_dir(src).with_context(|| format!("reading dir {}", src.display()))? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            std::fs::create_dir_all(&dst_path)
                .with_context(|| format!("creating dir {}", dst_path.display()))?;
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path).with_context(|| {
                format!("copying {} to {}", src_path.display(), dst_path.display())
            })?;
        }
    }
    Ok(())
}
