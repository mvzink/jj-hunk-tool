use anyhow::{Result, bail};
use std::process::Command;

pub use git_surgeon::diff::{check_supported, parse_diff};
pub use git_surgeon::hunk_id::assign_ids;

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
