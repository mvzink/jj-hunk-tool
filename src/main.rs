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
        /// Show only a brief preview of changed lines (no line numbers)
        #[arg(long)]
        compact: bool,
        /// Filter to a specific file
        #[arg(long)]
        file: Option<String>,
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
    /// Split selected hunks out of a revision (like jj split, but with hunk IDs)
    Split {
        /// Hunk IDs for the first (split-off) commit
        hunk_ids: Vec<String>,
        /// Revision to split
        #[arg(short, long, default_value = "@")]
        revision: String,
        /// Description for the first commit
        #[arg(short, long)]
        message: Option<String>,
        /// Create two parallel siblings instead of parent and child
        #[arg(short, long)]
        parallel: bool,
        /// Rebase selected changes onto these revisions
        #[arg(short, long, num_args = 1..)]
        onto: Vec<String>,
        /// Insert selected changes after these revisions
        #[arg(short = 'A', long, num_args = 1..)]
        insert_after: Vec<String>,
        /// Insert selected changes before these revisions
        #[arg(short = 'B', long, num_args = 1..)]
        insert_before: Vec<String>,
    },
    /// Rewrite hunks in a revision in-place (via jj diffedit --tool)
    Diffedit {
        /// Hunk IDs to keep (all others are removed from the revision)
        hunk_ids: Vec<String>,
        /// Revision to edit
        #[arg(short, long)]
        revision: Option<String>,
    },
    /// Undo selected hunks from a revision (like jj restore, but with hunk IDs)
    Restore {
        /// Hunk IDs to undo/restore
        hunk_ids: Vec<String>,
        /// Revision to restore from (source)
        #[arg(short, long)]
        from: Option<String>,
        /// Revision to restore into (destination)
        #[arg(short = 't', long)]
        into: Option<String>,
        /// Undo changes in this revision (default: @)
        #[arg(short, long)]
        changes_in: Option<String>,
        /// Preserve content (not diff) when rebasing descendants
        #[arg(long)]
        restore_descendants: bool,
    },
    /// Install the jj-surgeon skill into Claude Code (~/.claude/commands/)
    InstallSkill {
        /// Target directory (default: ~/.claude/commands)
        #[arg(long)]
        target: Option<String>,
    },
    /// Internal: JJ tool protocol handler (invoked by jj --tool)
    #[command(name = "_jj-tool", hide = true)]
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
            compact,
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

                if compact {
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
                } else {
                    let width = hunk.lines.len().to_string().len();
                    for (i, line) in hunk.lines.iter().enumerate() {
                        println!("{:>w$}:{line}", i + 1, w = width);
                    }
                }
                println!();
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
        Command::Split {
            hunk_ids,
            revision,
            message,
            parallel,
            onto,
            insert_after,
            insert_before,
        } => {
            let raw = get_jj_diff(&Some(revision.clone()))?;
            let hunks = parse_diff(&raw);
            let identified = assign_ids(&hunks);
            let specs = resolve_hunk_specs(&hunk_ids, &identified)?;
            let mut extra_args: Vec<String> = Vec::new();
            for rev in &onto {
                extra_args.push("-o".into());
                extra_args.push(rev.clone());
            }
            for rev in &insert_after {
                extra_args.push("-A".into());
                extra_args.push(rev.clone());
            }
            for rev in &insert_before {
                extra_args.push("-B".into());
                extra_args.push(rev.clone());
            }
            let extra_refs: Vec<&str> = extra_args.iter().map(|s| s.as_str()).collect();
            tool::split_hunks(
                &specs,
                Some(&revision),
                message.as_deref(),
                parallel,
                &extra_refs,
            )?;
        }
        Command::Diffedit { hunk_ids, revision } => {
            let rev = revision.as_deref().unwrap_or("@");
            let raw = get_jj_diff(&Some(rev.to_string()))?;
            let hunks = parse_diff(&raw);
            let identified = assign_ids(&hunks);
            let specs = resolve_hunk_specs(&hunk_ids, &identified)?;
            tool::diffedit_hunks(&specs, rev)?;
        }
        Command::Restore {
            hunk_ids,
            from,
            into,
            changes_in,
            restore_descendants,
        } => {
            // Determine which diff to inspect and what jj args to use.
            // Default (no flags) = --changes-in @
            let (raw, jj_args) = if let Some(ref ci) = changes_in {
                let raw = get_jj_diff(&Some(ci.clone()))?;
                let mut args = vec!["--changes-in".to_string(), ci.clone()];
                if restore_descendants {
                    args.push("--restore-descendants".into());
                }
                (raw, args)
            } else if from.is_some() || into.is_some() {
                let f = from.as_deref().unwrap_or("@");
                let t = into.as_deref().unwrap_or("@");
                let raw = diff::get_jj_diff_from_to(f, t)?;
                let mut args = vec!["--from".to_string(), f.to_string(), "--into".to_string(), t.to_string()];
                if restore_descendants {
                    args.push("--restore-descendants".into());
                }
                (raw, args)
            } else {
                // Default: --changes-in @
                let raw = get_jj_diff(&Some("@".to_string()))?;
                let mut args = vec!["--changes-in".to_string(), "@".to_string()];
                if restore_descendants {
                    args.push("--restore-descendants".into());
                }
                (raw, args)
            };
            let hunks = parse_diff(&raw);
            let identified = assign_ids(&hunks);
            let specs = resolve_hunk_specs(&hunk_ids, &identified)?;
            let jj_arg_refs: Vec<&str> = jj_args.iter().map(|s| s.as_str()).collect();
            tool::restore_hunks(&specs, &jj_arg_refs)?;
        }
        Command::InstallSkill { target } => {
            install_skill(target.as_deref())?;
        }
        Command::JjTool { left, right } => {
            tool::jj_tool_apply(&left, &right)?;
        }
    }

    Ok(())
}

use std::path::PathBuf;
use tool::HunkSpec;

const SKILL_MD: &str = include_str!("../skills/jj-surgeon/SKILL.md");
const REF_CONFLICT: &str = include_str!("../skills/jj-surgeon/references/conflict-resolution.md");
const REF_GIT_INTEROP: &str = include_str!("../skills/jj-surgeon/references/git-interop.md");
const REF_REVSET: &str = include_str!("../skills/jj-surgeon/references/revset-reference.md");
const REF_TEMPLATE: &str = include_str!("../skills/jj-surgeon/references/template-reference.md");

fn install_skill(target: Option<&str>) -> Result<()> {
    let skills_dir = match target {
        Some(t) => PathBuf::from(t),
        None => {
            let home = std::env::var("HOME")
                .map_err(|_| anyhow::anyhow!("HOME not set"))?;
            PathBuf::from(home).join(".claude").join("skills")
        }
    };

    let skill_dir = skills_dir.join("jj-surgeon");
    let refs_dir = skill_dir.join("references");
    std::fs::create_dir_all(&refs_dir)?;

    std::fs::write(skill_dir.join("SKILL.md"), SKILL_MD)?;
    std::fs::write(refs_dir.join("conflict-resolution.md"), REF_CONFLICT)?;
    std::fs::write(refs_dir.join("git-interop.md"), REF_GIT_INTEROP)?;
    std::fs::write(refs_dir.join("revset-reference.md"), REF_REVSET)?;
    std::fs::write(refs_dir.join("template-reference.md"), REF_TEMPLATE)?;

    println!("Installed jj-surgeon skill to {}", skill_dir.display());
    Ok(())
}

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
