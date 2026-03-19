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
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Hunks { revision } => {
            let raw = get_jj_diff(&revision)?;
            let hunks = parse_diff(&raw);
            let identified = assign_ids(&hunks);
            for (id, hunk) in &identified {
                println!(
                    "{id} {file}:{header}",
                    file = hunk.file,
                    header = hunk.header
                );
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
            let patch_text = git_surgeon::patch::build_patch(hunk);
            print!("{patch_text}");
        }
        Command::Patch {
            hunk_ids,
            revision,
            reverse,
        } => {
            let raw = get_jj_diff(&revision)?;
            let hunks = parse_diff(&raw);
            let identified = assign_ids(&hunks);
            for requested_id in &hunk_ids {
                let (_id, hunk) = identified
                    .iter()
                    .find(|(id, _)| id == requested_id)
                    .ok_or_else(|| anyhow::anyhow!("hunk not found: {requested_id}"))?;
                diff::check_supported(hunk, requested_id)?;
                if reverse {
                    let reversed = git_surgeon::patch::slice_hunk(hunk, 1, hunk.lines.len(), true)?;
                    let patch_text = git_surgeon::patch::build_patch(&reversed);
                    print!("{patch_text}");
                } else {
                    let patch_text = git_surgeon::patch::build_patch(hunk);
                    print!("{patch_text}");
                }
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
            let selected: Vec<_> = hunk_ids
                .iter()
                .map(|requested_id| {
                    identified
                        .iter()
                        .find(|(id, _)| id == requested_id)
                        .map(|(_, hunk)| *hunk)
                        .ok_or_else(|| anyhow::anyhow!("hunk not found: {requested_id}"))
                })
                .collect::<Result<_>>()?;
            tool::commit_hunks(&selected, &revision, message.as_deref())?;
        }
        Command::Discard { hunk_ids, revision } => {
            let raw = get_jj_diff(&revision)?;
            let hunks = parse_diff(&raw);
            let identified = assign_ids(&hunks);
            let selected: Vec<_> = hunk_ids
                .iter()
                .map(|requested_id| {
                    identified
                        .iter()
                        .find(|(id, _)| id == requested_id)
                        .map(|(_, hunk)| *hunk)
                        .ok_or_else(|| anyhow::anyhow!("hunk not found: {requested_id}"))
                })
                .collect::<Result<_>>()?;
            tool::discard_hunks(&selected, &revision)?;
        }
    }

    Ok(())
}
