use anyhow::Result;

use git_surgeon::diff::DiffHunk;

/// Commit selected hunks by generating a patch and applying it via jj.
pub fn commit_hunks(
    _hunks: &[&DiffHunk],
    _revision: &Option<String>,
    _message: Option<&str>,
) -> Result<()> {
    // TODO: implement using jj split or jj new + patch apply
    anyhow::bail!("commit command not yet implemented")
}

/// Discard selected hunks by generating a reverse patch and applying it.
pub fn discard_hunks(_hunks: &[&DiffHunk], _revision: &Option<String>) -> Result<()> {
    // TODO: implement using jj restore or patch apply
    anyhow::bail!("discard command not yet implemented")
}
