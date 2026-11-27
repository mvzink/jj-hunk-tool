use anyhow::{Result, bail};
use std::collections::HashSet;
use std::process::Command;

pub use git_surgeon::diff::parse_diff;
use git_surgeon::diff::{DiffHunk, check_supported};
pub use git_surgeon::hunk_id::assign_ids;

/// [`check_supported`], with coverage markers on both outcomes.
///
/// A hunk can carry preamble metadata that hunk-level operations cannot
/// represent (a rename, a copy, a mode change, a similarity index). Those hunks
/// are still *listed*, so every command that would move one must reject it
/// here: `patch/supported_hunk` and `error/unsupported_metadata` are the two
/// sides of that decision.
pub fn require_supported(hunk: &DiffHunk, id: &str) -> Result<()> {
    match check_supported(hunk, id) {
        Ok(()) => {
            crate::hit!(PatchSupportedHunk);
            Ok(())
        }
        Err(error) => {
            crate::hit!(ErrorUnsupportedMetadata);
            Err(error)
        }
    }
}

/// Is this side of a diff section absent, i.e. a `--- /dev/null` (added file) or
/// `+++ /dev/null` (deleted file) header?
///
/// `parse_diff` strips the leading slash, so the value it stores is the bare
/// `dev/null`; this accepts both spellings so that a comparison against the
/// `patch(1)` spelling cannot silently be dead code (which it was: `absorb`
/// tested `== "/dev/null"`, so its new-file branch had never run).
pub fn is_absent_side(path: &str) -> bool {
    path == "dev/null" || path == "/dev/null"
}

/// Get the jj workspace root directory.
pub fn get_repo_root() -> Result<std::path::PathBuf> {
    let output = Command::new("jj").args(["root", "--no-pager"]).output()?;
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
pub fn get_jj_annotations(
    revision: &str,
    file: &str,
    repo_root: &std::path::Path,
) -> Result<Vec<String>> {
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
pub fn get_ancestors_touching_file(
    source_rev: &str,
    file: &str,
    repo_root: &std::path::Path,
) -> Result<Vec<String>> {
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
            "op",
            "log",
            "--no-pager",
            "--no-graph",
            "--limit",
            "1",
            "-T",
            "self.id().short(16)",
        ])
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("jj op log failed: {stderr}");
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

/// Marker that starts each file's section in `jj diff --git` output.
const DIFF_FILE_MARKER: &[u8] = b"diff --git ";

/// Decode `jj diff --git` output, dropping file sections that are not valid UTF-8.
///
/// jj renders a non-UTF-8 file that contains no NUL byte as a text diff rather
/// than "Binary files differ", but such bytes cannot be represented in the
/// `String` the diff parser works on. Dropping the offending section treats the
/// file the same way jj's own binary files are treated: it is not listed and
/// cannot be selected. Decoding lossily instead would make the file selectable
/// and would let `split`/`squash`/`restore` write replacement characters back to
/// disk, silently corrupting it.
fn decode_diff(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut section_start = 0;
    let mut line_start = 0;
    let mut sections = 0;
    while line_start < bytes.len() {
        let line_end = bytes[line_start..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(bytes.len(), |i| line_start + i + 1);
        if line_start > section_start && bytes[line_start..line_end].starts_with(DIFF_FILE_MARKER) {
            push_diff_section(&mut out, &bytes[section_start..line_start]);
            sections += 1;
            section_start = line_start;
        }
        line_start = line_end;
    }
    if !bytes.is_empty() {
        push_diff_section(&mut out, &bytes[section_start..]);
        sections += 1;
    }
    if sections > 1 {
        crate::hit!(DiffSectionsMultiple);
    }
    out
}

/// Append one file's diff section to `out` if it is valid UTF-8.
fn push_diff_section(out: &mut String, section: &[u8]) {
    match std::str::from_utf8(section) {
        Ok(text) => {
            crate::hit!(DiffSectionDecoded);
            out.push_str(text);
        }
        Err(_) => {
            crate::hit!(DiffSectionUndecodableSkipped);
            let header = String::from_utf8_lossy(section);
            let header = header.lines().next().unwrap_or("");
            eprintln!("warning: skipping non-UTF-8 file (treated as binary): {header}");
        }
    }
}

/// Run `jj diff --git` for the given revision and return its text diff.
/// File sections that are not valid UTF-8 are omitted (see [`decode_diff`]).
pub fn get_jj_diff(revision: &Option<String>, debug: bool) -> Result<String> {
    let mut cmd = Command::new("jj");
    cmd.args(["diff", "--git", "--no-pager"]);
    if let Some(rev) = revision {
        cmd.args(["-r", rev]);
    }
    if debug {
        eprintln!(
            "debug: running jj diff --git --no-pager{}",
            revision
                .as_ref()
                .map(|r| format!(" -r {r}"))
                .unwrap_or_default()
        );
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
    Ok(decode_diff(&output.stdout))
}

/// Run `jj diff --git --from FROM --to TO` and return its text diff.
/// File sections that are not valid UTF-8 are omitted (see [`decode_diff`]).
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
    Ok(decode_diff(&output.stdout))
}
