mod diff;
mod hunk_id;
mod patch;
mod tool;

use anyhow::Result;
use clap::{Parser, Subcommand};

use diff::{assign_ids, get_jj_diff, parse_diff};

#[derive(Parser)]
#[command(name = "jj-hunk-tool", version, about = "Hunk-level operations for jj")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List hunks in a diff with stable IDs
    Hunks {
        /// Revision to diff (default: working copy)
        #[arg(short, long)]
        revision: Option<String>,
        /// Show all lines with line numbers
        #[arg(long)]
        full: bool,
        /// Filter to a specific file
        #[arg(long)]
        file: Option<String>,
    },
    /// Show details of a specific hunk
    Show {
        /// Hunk ID to show
        hunk_id: String,
        /// Revision to diff
        #[arg(short, long)]
        revision: Option<String>,
    },
    /// Output a patch for selected hunks
    Patch {
        /// Hunk IDs to include
        hunk_ids: Vec<String>,
        /// Revision to diff
        #[arg(short, long)]
        revision: Option<String>,
        /// Reverse the patch (for discarding)
        #[arg(long)]
        reverse: bool,
    },
    /// Commit selected hunks into a new change
    Commit {
        /// Hunk IDs to commit
        hunk_ids: Vec<String>,
        /// Revision to split from
        #[arg(short, long)]
        revision: Option<String>,
        /// Commit message
        #[arg(short, long)]
        message: Option<String>,
    },
    /// Discard selected hunks from the working copy
    Discard {
        /// Hunk IDs to discard
        hunk_ids: Vec<String>,
        /// Revision to discard from
        #[arg(short, long)]
        revision: Option<String>,
    },
    /// Rewrite hunks in a revision in-place (via jj diffedit --tool)
    Diffedit {
        /// Hunk IDs to keep (all others are removed from the revision)
        hunk_ids: Vec<String>,
        /// Revision to edit
        #[arg(short, long)]
        revision: Option<String>,
    },
    /// Restore selected hunks from a revision (via jj restore --tool)
    Restore {
        /// Hunk IDs to restore
        hunk_ids: Vec<String>,
        /// Source revision to restore from
        #[arg(long)]
        from: String,
        /// Target revision to restore into (default: @)
        #[arg(long)]
        to: Option<String>,
    },
    /// Internal: JJ tool protocol handler (invoked by jj --tool)
    #[command(name = "_jj-tool")]
    JjTool {
        /// Left directory (parent/base state)
        left: String,
        /// Right directory (current state, writable)
        right: String,
    },
}

fn main() -> Result<()> {
    // Prevent any child process (jj, patch, etc.) from opening an interactive editor.
    // SAFETY: this runs at the start of main before any threads are spawned.
    unsafe { std::env::set_var("EDITOR", "true") };

    let cli = Cli::parse();

    match cli.command {
        Command::Hunks {
            revision,
            full,
            file,
        } => {
            let raw = get_jj_diff(&revision)?;
            let hunks = parse_diff(&raw);
            let identified = assign_ids(&hunks);

            let max_preview_lines = 4;

            for (id, hunk) in &identified {
                if let Some(ref f) = file
                    && &hunk.file != f
                {
                    continue;
                }

                let additions = hunk.lines.iter().filter(|l| l.starts_with('+')).count();
                let deletions = hunk.lines.iter().filter(|l| l.starts_with('-')).count();

                let func_ctx = hunk
                    .header
                    .find("@@ ")
                    .and_then(|start| {
                        let rest = &hunk.header[start + 3..];
                        rest.find("@@ ").map(|end| rest[end + 3..].trim())
                    })
                    .unwrap_or("");

                let func_part = if func_ctx.is_empty() {
                    String::new()
                } else {
                    format!(" {func_ctx}")
                };

                println!("{id} {}{func_part} (+{additions} -{deletions})", hunk.file);

                if full {
                    let width = hunk.lines.len().to_string().len();
                    for (i, line) in hunk.lines.iter().enumerate() {
                        println!("{:>w$}:{line}", i + 1, w = width);
                    }
                } else {
                    let changed: Vec<&String> = hunk
                        .lines
                        .iter()
                        .filter(|l| l.starts_with('+') || l.starts_with('-'))
                        .collect();
                    let show = changed.len().min(max_preview_lines);
                    for line in &changed[..show] {
                        println!("  {line}");
                    }
                    if changed.len() > max_preview_lines {
                        println!("  ... (+{} more lines)", changed.len() - max_preview_lines);
                    }
                }
                println!();
            }
        }
        Command::Show { hunk_id, revision } => {
            let raw = get_jj_diff(&revision)?;
            let hunks = parse_diff(&raw);
            let identified = assign_ids(&hunks);
            let (_id, hunk) = identified
                .iter()
                .find(|(id, _)| id == &hunk_id)
                .ok_or_else(|| anyhow::anyhow!("hunk not found: {hunk_id}"))?;
            println!("{}", hunk.header);
            let width = hunk.lines.len().to_string().len();
            for (i, line) in hunk.lines.iter().enumerate() {
                println!("{:>w$}:{line}", i + 1, w = width);
            }
        }
        Command::Patch {
            hunk_ids,
            revision,
            reverse,
        } => {
            let raw = get_jj_diff(&revision)?;
            let hunks = parse_diff(&raw);
            let identified = assign_ids(&hunks);
            for raw_spec in &hunk_ids {
                let (id, ranges) = parse_id_range(raw_spec)?;
                let (_id, hunk) = identified
                    .iter()
                    .find(|(hid, _)| hid == id)
                    .ok_or_else(|| anyhow::anyhow!("hunk not found: {id}"))?;
                diff::check_supported(hunk, id)?;
                let patched = if !ranges.is_empty() {
                    git_surgeon::patch::slice_hunk_multi(hunk, &ranges, reverse)?
                } else if reverse {
                    git_surgeon::patch::slice_hunk(hunk, 1, hunk.lines.len(), true)?
                } else {
                    (*hunk).clone()
                };
                print!("{}", git_surgeon::patch::build_patch(&patched));
            }
        }
        Command::Commit {
            hunk_ids,
            revision,
            message,
        } => {
            let raw = get_jj_diff(&revision)?;
            let hunks = parse_diff(&raw);
            let identified = assign_ids(&hunks);
            let specs = resolve_hunk_specs(&hunk_ids, &identified)?;
            tool::commit_hunks(&specs, &revision, message.as_deref())?;
        }
        Command::Discard { hunk_ids, revision } => {
            let raw = get_jj_diff(&revision)?;
            let hunks = parse_diff(&raw);
            let identified = assign_ids(&hunks);
            let specs = resolve_hunk_specs(&hunk_ids, &identified)?;
            tool::discard_hunks(&specs, &revision)?;
        }
        Command::Diffedit { hunk_ids, revision } => {
            let rev = revision.as_deref().unwrap_or("@");
            let raw = get_jj_diff(&Some(rev.to_string()))?;
            let hunks = parse_diff(&raw);
            let identified = assign_ids(&hunks);
            let specs = resolve_hunk_specs(&hunk_ids, &identified)?;
            tool::diffedit_hunks(&specs, rev)?;
        }
        Command::Restore { hunk_ids, from, to } => {
            let raw = get_jj_diff(&Some(from.clone()))?;
            let hunks = parse_diff(&raw);
            let identified = assign_ids(&hunks);
            let specs = resolve_hunk_specs(&hunk_ids, &identified)?;
            tool::restore_hunks(&specs, &from, to.as_deref())?;
        }
        Command::JjTool { left, right } => {
            tool::jj_tool_apply(&left, &right)?;
        }
    }

    Ok(())
}

use tool::HunkSpec;

/// Resolve hunk ID specs (with optional line ranges) against identified hunks.
fn resolve_hunk_specs<'a>(
    raw_specs: &[String],
    identified: &'a [(String, &'a git_surgeon::diff::DiffHunk)],
) -> Result<Vec<HunkSpec<'a>>> {
    let mut specs = Vec::new();
    for raw in raw_specs {
        let (id, ranges) = parse_id_range(raw)?;
        let (matched_id, hunk) = identified
            .iter()
            .find(|(hid, _)| hid == id)
            .ok_or_else(|| anyhow::anyhow!("hunk not found: {id}"))?;
        specs.push((matched_id.as_str(), *hunk, ranges));
    }
    Ok(specs)
}

/// Parse an ID spec that may contain inline range suffixes.
/// Supports: "id", "id:5", "id:1-11", "id:2,5-6,34" (comma-separated).
/// Returns (id, vector of ranges). Empty vector means "whole hunk".
fn parse_id_range(raw: &str) -> Result<(&str, Vec<(usize, usize)>)> {
    let Some((id, range_str)) = raw.split_once(':') else {
        return Ok((raw, Vec::new()));
    };
    let mut ranges = Vec::new();
    for part in range_str.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (start, end) = if let Some((a, b)) = part.split_once('-') {
            let start: usize = a
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid start number in '{raw}'"))?;
            let end: usize = b
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid end number in '{raw}'"))?;
            (start, end)
        } else {
            let n: usize = part
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid line number in '{raw}'"))?;
            (n, n)
        };
        if start == 0 || end == 0 || start > end {
            anyhow::bail!("range must be 1-based and start <= end in '{raw}'");
        }
        ranges.push((start, end));
    }
    Ok((id, ranges))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_id_range_plain_id() {
        let (id, ranges) = parse_id_range("abc1234").unwrap();
        assert_eq!(id, "abc1234");
        assert!(ranges.is_empty());
    }

    #[test]
    fn parse_id_range_single_line() {
        let (id, ranges) = parse_id_range("abc:5").unwrap();
        assert_eq!(id, "abc");
        assert_eq!(ranges, vec![(5, 5)]);
    }

    #[test]
    fn parse_id_range_range() {
        let (id, ranges) = parse_id_range("abc:3-10").unwrap();
        assert_eq!(id, "abc");
        assert_eq!(ranges, vec![(3, 10)]);
    }

    #[test]
    fn parse_id_range_multiple_ranges() {
        let (id, ranges) = parse_id_range("abc:1-3,7-9").unwrap();
        assert_eq!(id, "abc");
        assert_eq!(ranges, vec![(1, 3), (7, 9)]);
    }

    #[test]
    fn parse_id_range_zero_start_rejected() {
        assert!(parse_id_range("abc:0-5").is_err());
    }

    #[test]
    fn parse_id_range_zero_end_rejected() {
        assert!(parse_id_range("abc:1-0").is_err());
    }

    #[test]
    fn parse_id_range_reversed_rejected() {
        assert!(parse_id_range("abc:5-3").is_err());
    }

    #[test]
    fn parse_id_range_non_numeric_rejected() {
        assert!(parse_id_range("abc:xyz").is_err());
    }

    #[test]
    fn parse_id_range_empty_parts_skipped() {
        let (id, ranges) = parse_id_range("abc:1-3,,7-9").unwrap();
        assert_eq!(id, "abc");
        assert_eq!(ranges, vec![(1, 3), (7, 9)]);
    }

    #[test]
    fn parse_id_range_trailing_comma() {
        let (id, ranges) = parse_id_range("abc:1-3,").unwrap();
        assert_eq!(id, "abc");
        assert_eq!(ranges, vec![(1, 3)]);
    }
}
