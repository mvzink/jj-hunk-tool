use anyhow::{Result, bail};
use std::collections::HashSet;
use std::process::Command;

pub use git_surgeon::diff::{check_supported, parse_diff};
pub use git_surgeon::hunk_id::assign_ids;

/// Get the jj workspace root directory.
pub fn get_repo_root() -> Result<std::path::PathBuf> {
    let output = Command::new("jj")
        .args(["root", "--no-pager"])
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("jj root failed: {stderr}");
    }
    let root = String::from_utf8(output.stdout)?.trim().to_string();
    Ok(std::path::PathBuf::from(root))
}

/// Get per-line annotation (change IDs) for a file at a given revision.
/// File path must be repo-root-relative.
/// Returns one change ID per line of the file.
pub fn get_jj_annotations(revision: &str, file: &str, repo_root: &std::path::Path) -> Result<Vec<String>> {
    let output = Command::new("jj")
        .args([
            "file",
            "annotate",
            "--no-pager",
            "-r",
            revision,
            "-T",
            "commit.change_id().shortest(8) ++ \"\\n\"",
            file,
        ])
        .current_dir(repo_root)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("jj file annotate failed: {stderr}");
    }
    let stdout = String::from_utf8(output.stdout)?;
    Ok(stdout.lines().map(|l| l.to_string()).collect())
}

/// Get mutable ancestors and their descriptions in a single jj call.
/// Returns (set of change IDs, map of change ID → first line of description).
pub fn get_mutable_ancestors_with_descriptions(
    source_rev: &str,
) -> Result<(HashSet<String>, std::collections::HashMap<String, String>)> {
    use std::collections::HashMap;
    let revset = format!("immutable_heads()..({source_rev}-)");
    let output = Command::new("jj")
        .args([
            "log",
            "--no-pager",
            "--no-graph",
            "-r",
            &revset,
            "-T",
            r#"change_id.shortest(8) ++ "\t" ++ description.first_line() ++ "\n""#,
        ])
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("jj log failed: {stderr}");
    }
    let stdout = String::from_utf8(output.stdout)?;
    let mut ids = HashSet::new();
    let mut descs = HashMap::new();
    for line in stdout.lines() {
        if let Some((id, desc)) = line.split_once('\t') {
            ids.insert(id.to_string());
            descs.insert(id.to_string(), desc.to_string());
        }
    }
    Ok((ids, descs))
}

/// Get mutable ancestors that touched a specific file, ordered most-recent-first.
/// File path must be repo-root-relative.
pub fn get_ancestors_touching_file(source_rev: &str, file: &str, repo_root: &std::path::Path) -> Result<Vec<String>> {
    let revset = format!("(immutable_heads()..({source_rev}-)) & files(\"{file}\")");
    let output = Command::new("jj")
        .args([
            "log",
            "--no-pager",
            "--no-graph",
            "-r",
            &revset,
            "-T",
            "change_id.shortest(8) ++ \"\\n\"",
        ])
        .current_dir(repo_root)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("jj log failed: {stderr}");
    }
    let stdout = String::from_utf8(output.stdout)?;
    Ok(stdout.lines().map(|l| l.to_string()).collect())
}

/// Get the current jj operation ID.
pub fn get_current_op_id() -> Result<String> {
    let output = Command::new("jj")
        .args([
            "op", "log", "--no-pager", "--no-graph", "--limit", "1",
            "-T", "self.id().short(16)",
        ])
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("jj op log failed: {stderr}");
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

/// Run `jj diff --git` for the given revision and return the raw output.
pub fn get_jj_diff(revision: &Option<String>, debug: bool) -> Result<String> {
    let mut cmd = Command::new("jj");
    cmd.args(["diff", "--git", "--no-pager"]);
    if let Some(rev) = revision {
        cmd.args(["-r", rev]);
    }
    if debug {
        eprintln!("debug: running jj diff --git --no-pager{}", revision.as_ref().map(|r| format!(" -r {r}")).unwrap_or_default());
    }
    let output = cmd.output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if debug {
            eprintln!("debug: jj diff failed with stderr: {stderr}");
        }
        bail!("jj diff failed: {stderr}");
    }
    if debug {
        eprintln!("debug: jj diff returned {} bytes", output.stdout.len());
    }
    Ok(String::from_utf8(output.stdout)?)
}

/// Run `jj diff --git --from FROM --to TO` and return the raw output.
pub fn get_jj_diff_from_to(from: &str, to: &str, debug: bool) -> Result<String> {
    if debug {
        eprintln!("debug: running jj diff --git --no-pager --from {from} --to {to}");
    }
    let output = Command::new("jj")
        .args(["diff", "--git", "--no-pager", "--from", from, "--to", to])
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if debug {
            eprintln!("debug: jj diff failed with stderr: {stderr}");
        }
        bail!("jj diff failed: {stderr}");
    }
    if debug {
        eprintln!("debug: jj diff returned {} bytes", output.stdout.len());
    }
    Ok(String::from_utf8(output.stdout)?)
}
