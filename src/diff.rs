use anyhow::{Result, bail};
use std::collections::HashSet;
use std::process::Command;

pub use git_surgeon::diff::{check_supported, parse_diff};
pub use git_surgeon::hunk_id::assign_ids;

/// Get per-line annotation (change IDs) for a file at a given revision.
/// Returns one change ID per line of the file.
pub fn get_jj_annotations(revision: &str, file: &str) -> Result<Vec<String>> {
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
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("jj file annotate failed: {stderr}");
    }
    let stdout = String::from_utf8(output.stdout)?;
    Ok(stdout.lines().map(|l| l.to_string()).collect())
}

/// Get the set of mutable ancestor change IDs between immutable_heads() and the
/// given revision's parents.
pub fn get_mutable_ancestors(source_rev: &str) -> Result<HashSet<String>> {
    let revset = format!("immutable_heads()..({source_rev}-)");
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
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("jj log failed: {stderr}");
    }
    let stdout = String::from_utf8(output.stdout)?;
    Ok(stdout.lines().map(|l| l.to_string()).collect())
}

/// Get the description (first line) for a change ID.
pub fn get_change_description(change_id: &str) -> Result<String> {
    let output = Command::new("jj")
        .args([
            "log",
            "--no-pager",
            "--no-graph",
            "-r",
            change_id,
            "-T",
            "description.first_line()",
        ])
        .output()?;
    if !output.status.success() {
        return Ok(String::new());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

/// Run `jj diff --git` for the given revision and return the raw output.
pub fn get_jj_diff(revision: &Option<String>) -> Result<String> {
    let mut cmd = Command::new("jj");
    cmd.args(["diff", "--git", "--no-pager"]);
    if let Some(rev) = revision {
        cmd.args(["-r", rev]);
    }
    let output = cmd.output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("jj diff failed: {stderr}");
    }
    Ok(String::from_utf8(output.stdout)?)
}

/// Run `jj diff --git --from FROM --to TO` and return the raw output.
pub fn get_jj_diff_from_to(from: &str, to: &str) -> Result<String> {
    let output = Command::new("jj")
        .args(["diff", "--git", "--no-pager", "--from", from, "--to", to])
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("jj diff failed: {stderr}");
    }
    Ok(String::from_utf8(output.stdout)?)
}
