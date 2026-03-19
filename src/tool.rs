use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

use git_surgeon::diff::DiffHunk;

const PATCH_ENV_VAR: &str = "JJ_HUNK_TOOL_PATCH";

/// Commit selected hunks by generating a patch and applying it via jj.
pub fn commit_hunks(
    _hunks: &[&DiffHunk],
    _revision: &Option<String>,
    _message: Option<&str>,
) -> Result<()> {
    // TODO: implement using jj split --tool
    bail!("commit command not yet implemented")
}

/// Discard selected hunks by generating a reverse patch and applying it.
pub fn discard_hunks(_hunks: &[&DiffHunk], _revision: &Option<String>) -> Result<()> {
    // TODO: implement using jj restore --tool
    bail!("discard command not yet implemented")
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
/// Removes files in dst that don't exist in src, copies all files from src to dst.
fn reset_dir_to(src: &Path, dst: &Path) -> Result<()> {
    // Remove all files in dst
    remove_dir_contents(dst)?;

    // Copy all files from src to dst
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
