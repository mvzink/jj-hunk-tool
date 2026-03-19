use anyhow::Result;

use crate::diff::Hunk;
use crate::patch;

/// Commit selected hunks by generating a patch and applying it via jj.
pub fn commit_hunks(
    hunks: &[&Hunk],
    revision: &Option<String>,
    message: Option<&str>,
) -> Result<()> {
    let patch_content = patch::build_patch(hunks, false);
    apply_patch_to_new_commit(&patch_content, revision, message)
}

/// Discard selected hunks by generating a reverse patch and applying it.
pub fn discard_hunks(hunks: &[&Hunk], revision: &Option<String>) -> Result<()> {
    let patch_content = patch::build_patch(hunks, true);
    apply_reverse_patch(&patch_content, revision)
}

fn apply_patch_to_new_commit(
    _patch: &str,
    _revision: &Option<String>,
    _message: Option<&str>,
) -> Result<()> {
    // TODO: implement using jj split or jj new + patch apply
    anyhow::bail!("commit command not yet implemented")
}

fn apply_reverse_patch(_patch: &str, _revision: &Option<String>) -> Result<()> {
    // TODO: implement using jj restore or patch apply
    anyhow::bail!("discard command not yet implemented")
}
