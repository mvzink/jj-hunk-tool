use crate::diff::Hunk;

/// Build a unified diff patch from the selected hunks.
/// If `reverse` is true, swap +/- lines for use in discarding changes.
pub fn build_patch(hunks: &[&Hunk], reverse: bool) -> String {
    let mut output = String::new();
    let mut last_file: Option<&str> = None;

    for hunk in hunks {
        // Emit file header if we're in a new file
        if last_file != Some(&hunk.file_path) {
            if reverse {
                // Swap --- and +++ for reverse patches
                for line in hunk.file_header.lines() {
                    if let Some(rest) = line.strip_prefix("--- a/") {
                        output.push_str(&format!("+++ a/{rest}\n"));
                    } else if let Some(rest) = line.strip_prefix("+++ b/") {
                        output.push_str(&format!("--- b/{rest}\n"));
                    } else {
                        output.push_str(line);
                        output.push('\n');
                    }
                }
            } else {
                output.push_str(&hunk.file_header);
            }
            last_file = Some(&hunk.file_path);
        }

        if reverse {
            // Reverse each line in the hunk content
            for line in hunk.content.lines() {
                if let Some(rest) = line.strip_prefix('+') {
                    output.push('-');
                    output.push_str(rest);
                } else if let Some(rest) = line.strip_prefix('-') {
                    output.push('+');
                    output.push_str(rest);
                } else {
                    output.push_str(line);
                }
                output.push('\n');
            }
        } else {
            output.push_str(&hunk.content);
        }
    }

    output
}
