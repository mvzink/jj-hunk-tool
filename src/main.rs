mod diff;
mod hunk_id;
mod patch;
mod tool;

use anyhow::Result;
use clap::{Parser, Subcommand};

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
            let diff = diff::get_diff(&revision)?;
            let hunks = diff::parse_hunks(&diff)?;
            for hunk in &hunks {
                let id = hunk_id::compute_id(hunk);
                println!("{} {}:{}", id, hunk.file_path, hunk.header);
            }
        }
        Command::Show { hunk_id, revision } => {
            let diff = diff::get_diff(&revision)?;
            let hunks = diff::parse_hunks(&diff)?;
            let hunk = hunks
                .iter()
                .find(|h| hunk_id::compute_id(h) == hunk_id)
                .ok_or_else(|| anyhow::anyhow!("hunk not found: {hunk_id}"))?;
            print!("{}", hunk.content);
        }
        Command::Patch {
            hunk_ids,
            revision,
            reverse,
        } => {
            let diff = diff::get_diff(&revision)?;
            let hunks = diff::parse_hunks(&diff)?;
            let selected = diff::select_hunks(&hunks, &hunk_ids)?;
            let output = patch::build_patch(&selected, reverse);
            print!("{output}");
        }
        Command::Commit {
            hunk_ids,
            revision,
            message,
        } => {
            let diff = diff::get_diff(&revision)?;
            let hunks = diff::parse_hunks(&diff)?;
            let selected = diff::select_hunks(&hunks, &hunk_ids)?;
            tool::commit_hunks(&selected, &revision, message.as_deref())?;
        }
        Command::Discard { hunk_ids, revision } => {
            let diff = diff::get_diff(&revision)?;
            let hunks = diff::parse_hunks(&diff)?;
            let selected = diff::select_hunks(&hunks, &hunk_ids)?;
            tool::discard_hunks(&selected, &revision)?;
        }
    }

    Ok(())
}
