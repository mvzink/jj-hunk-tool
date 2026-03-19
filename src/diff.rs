use anyhow::{Result, bail};
use std::process::Command;

use crate::hunk_id;

/// A parsed hunk from a git-format diff.
#[derive(Debug, Clone)]
pub struct Hunk {
    /// The file path this hunk applies to.
    pub file_path: String,
    /// The @@ header line (e.g. "@@ -1,3 +1,4 @@").
    pub header: String,
    /// The full content of the hunk including header and diff lines.
    pub content: String,
    /// The diff header for this file (--- and +++ lines).
    pub file_header: String,
}

/// Run `jj diff --git` for the given revision and return the raw output.
pub fn get_diff(revision: &Option<String>) -> Result<String> {
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

/// Parse a git-format diff into individual hunks.
pub fn parse_hunks(diff: &str) -> Result<Vec<Hunk>> {
    let mut hunks = Vec::new();
    let mut current_file: Option<String> = None;
    let mut current_file_header = String::new();
    let mut current_header: Option<String> = None;
    let mut current_content = String::new();

    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            // Flush previous hunk
            flush_hunk(
                &mut hunks,
                &current_file,
                &current_file_header,
                &current_header,
                &current_content,
            );
            current_header = None;
            current_content.clear();

            // Parse file path from "diff --git a/path b/path"
            let path = parse_diff_path(line);
            current_file = Some(path);
            current_file_header.clear();
            current_file_header.push_str(line);
            current_file_header.push('\n');
        } else if line.starts_with("--- ") || line.starts_with("+++ ") {
            current_file_header.push_str(line);
            current_file_header.push('\n');
        } else if line.starts_with("@@ ") {
            // Flush previous hunk
            flush_hunk(
                &mut hunks,
                &current_file,
                &current_file_header,
                &current_header,
                &current_content,
            );

            current_header = Some(line.to_string());
            current_content.clear();
            current_content.push_str(line);
            current_content.push('\n');
        } else if line.starts_with('+')
            || line.starts_with('-')
            || line.starts_with(' ')
            || line == "\\ No newline at end of file"
        {
            current_content.push_str(line);
            current_content.push('\n');
        } else if line.starts_with("index ")
            || line.starts_with("old mode ")
            || line.starts_with("new mode ")
            || line.starts_with("new file mode ")
            || line.starts_with("deleted file mode ")
            || line.starts_with("similarity index ")
            || line.starts_with("rename from ")
            || line.starts_with("rename to ")
            || line.starts_with("copy from ")
            || line.starts_with("copy to ")
        {
            current_file_header.push_str(line);
            current_file_header.push('\n');
        }
    }

    // Flush last hunk
    flush_hunk(
        &mut hunks,
        &current_file,
        &current_file_header,
        &current_header,
        &current_content,
    );

    Ok(hunks)
}

fn flush_hunk(
    hunks: &mut Vec<Hunk>,
    file: &Option<String>,
    file_header: &str,
    header: &Option<String>,
    content: &str,
) {
    if let (Some(file), Some(header)) = (file, header)
        && !content.is_empty()
    {
        hunks.push(Hunk {
            file_path: file.clone(),
            header: header.clone(),
            content: content.to_string(),
            file_header: file_header.to_string(),
        });
    }
}

/// Parse the file path from a `diff --git a/path b/path` line.
fn parse_diff_path(line: &str) -> String {
    // "diff --git a/foo b/foo" -> "foo"
    let rest = line.strip_prefix("diff --git ").unwrap_or(line);
    if let Some(idx) = rest.find(" b/") {
        rest[idx + 3..].to_string()
    } else {
        rest.to_string()
    }
}

/// Select hunks by their IDs, returning an error if any ID is not found.
pub fn select_hunks<'a>(hunks: &'a [Hunk], ids: &[String]) -> Result<Vec<&'a Hunk>> {
    let mut selected = Vec::new();
    for id in ids {
        let hunk = hunks
            .iter()
            .find(|h| hunk_id::compute_id(h) == *id)
            .ok_or_else(|| anyhow::anyhow!("hunk not found: {id}"))?;
        selected.push(hunk);
    }
    Ok(selected)
}
