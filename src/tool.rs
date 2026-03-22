use std::io::Write as _;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use console::Style;
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::as_24_bit_terminal_escaped;

use git_surgeon::diff::DiffHunk;

const PATCH_ENV_VAR: &str = "JJ_HUNK_TOOL_PATCH";
const REVERSE_ENV_VAR: &str = "JJ_HUNK_TOOL_REVERSE";

/// Display a hunk with syntax highlighting, diff colors, and absolute line numbers.
/// Returns the formatted string (no ANSI codes if `color` is false).
fn format_hunk_display(hunk: &DiffHunk, color: bool) -> String {
    let mut out = String::new();

    // Parse old/new start lines from @@ header
    let (old_start, new_start) = parse_header_ranges(&hunk.header);

    // Set up syntax highlighting
    let ss = SyntaxSet::load_defaults_newlines();
    let ts = ThemeSet::load_defaults();
    let syntax = ss
        .find_syntax_by_extension(
            Path::new(&hunk.file)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or(""),
        )
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    let theme = &ts.themes["base16-ocean.dark"];
    let mut highlighter = HighlightLines::new(syntax, theme);

    let add_style = Style::new().green();
    let del_style = Style::new().red();
    let ctx_style = Style::new().dim();
    let lineno_style = Style::new().dim();

    let mut old_line = old_start;
    let mut new_line = new_start;

    // Compute max line number width for alignment
    let max_line = old_start
        .max(new_start)
        + hunk.lines.len();
    let width = max_line.to_string().len();

    for line in &hunk.lines {
        let (prefix, content, lineno) = if let Some(rest) = line.strip_prefix('+') {
            let ln = new_line;
            new_line += 1;
            ("+", rest, ln)
        } else if let Some(rest) = line.strip_prefix('-') {
            let ln = old_line;
            old_line += 1;
            ("-", rest, ln)
        } else if let Some(rest) = line.strip_prefix(' ') {
            let ln = old_line;
            old_line += 1;
            new_line += 1;
            (" ", rest, ln)
        } else {
            // Shouldn't happen, but handle gracefully
            let ln = old_line;
            old_line += 1;
            new_line += 1;
            (" ", line.as_str(), ln)
        };

        if color {
            // Syntax-highlight the content
            let content_with_nl = format!("{content}\n");
            let highlighted = if prefix != "-" {
                // Syntax highlight additions and context
                highlighter
                    .highlight_line(&content_with_nl, &ss)
                    .map(|ranges| {
                        let mut s = as_24_bit_terminal_escaped(&ranges, false);
                        // Strip trailing newline from highlighted output
                        if s.ends_with('\n') {
                            s.pop();
                        }
                        // Reset at end
                        s.push_str("\x1b[0m");
                        s
                    })
                    .unwrap_or_else(|_| content.to_string())
            } else {
                // For deleted lines, just use the raw content (red coloring applied to whole line)
                content.to_string()
            };

            let formatted_lineno = lineno_style.apply_to(format!("{lineno:>width$}"));
            let formatted_line = match prefix {
                "+" => format!(
                    "{formatted_lineno} {} {highlighted}",
                    add_style.apply_to("+")
                ),
                "-" => format!(
                    "{formatted_lineno} {} {}",
                    del_style.apply_to("-"),
                    del_style.apply_to(content)
                ),
                _ => format!(
                    "{formatted_lineno} {} {highlighted}",
                    ctx_style.apply_to(" ")
                ),
            };
            out.push_str(&formatted_line);
        } else {
            // Plain text: absolute line numbers, no color
            out.push_str(&format!("{lineno:>width$}:{prefix}{content}"));
        }
        out.push('\n');
    }

    out
}

/// Parse both old and new start lines from a @@ header.
/// "@@ -old_start,count +new_start,count @@" → (old_start, new_start)
fn parse_header_ranges(header: &str) -> (usize, usize) {
    let header = header.trim();
    let after_at = match header.strip_prefix("@@ -") {
        Some(s) => s,
        None => return (1, 1),
    };

    // Parse old range
    let old_end = after_at.find(' ').unwrap_or(after_at.len());
    let old_range_str = &after_at[..old_end];
    let old_start = old_range_str
        .split_once(',')
        .map(|(s, _)| s)
        .unwrap_or(old_range_str)
        .parse::<usize>()
        .unwrap_or(1);

    // Parse new range (after "+")
    let rest = &after_at[old_end..];
    let new_start = rest
        .find('+')
        .and_then(|pos| {
            let after_plus = &rest[pos + 1..];
            let end = after_plus.find(' ').unwrap_or(after_plus.len());
            let new_range_str = &after_plus[..end];
            new_range_str
                .split_once(',')
                .map(|(s, _)| s)
                .unwrap_or(new_range_str)
                .parse::<usize>()
                .ok()
        })
        .unwrap_or(1);

    (old_start, new_start)
}

/// A hunk spec: (hunk_id, hunk, line_ranges).
pub type HunkSpec<'a> = (&'a str, &'a DiffHunk, Vec<(usize, usize)>);

/// Build a combined patch from selected hunks with optional line ranges.
pub fn build_combined_patch(specs: &[HunkSpec<'_>], reverse: bool) -> Result<String> {
    let mut combined = String::new();
    for (id, hunk, ranges) in specs {
        git_surgeon::diff::check_supported(hunk, id)?;
        let patched = if !ranges.is_empty() {
            git_surgeon::patch::slice_hunk_multi(hunk, ranges, reverse)?
        } else if reverse {
            git_surgeon::patch::slice_hunk(hunk, 1, hunk.lines.len(), true)?
        } else {
            (*hunk).clone()
        };
        combined.push_str(&git_surgeon::patch::build_patch(&patched));
    }
    Ok(combined)
}

/// Write a temp jj config TOML that defines jj-hunk-tool as a merge tool
/// and overrides the user's editor to prevent interactive prompts.
fn write_tool_config(exe: &Path) -> Result<tempfile::NamedTempFile> {
    let mut config_file = tempfile::Builder::new()
        .suffix(".toml")
        .tempfile()
        .context("creating temp config file")?;
    write!(
        config_file,
        "[ui]\neditor = \"true\"\n\n[merge-tools.jj-hunk-tool]\nprogram = {exe:?}\nedit-args = [\"_jj-tool\", \"$left\", \"$right\"]\n",
        exe = exe.display().to_string(),
    )
    .context("writing config")?;
    Ok(config_file)
}

/// Run a jj command with our tool configured.
fn run_jj_with_tool(jj_args: &[&str], patch_content: &str, reverse: bool) -> Result<()> {
    let exe = std::env::current_exe().context("finding own executable")?;

    let mut patch_file = tempfile::NamedTempFile::new().context("creating temp patch file")?;
    patch_file
        .write_all(patch_content.as_bytes())
        .context("writing patch")?;

    let config_file = write_tool_config(&exe)?;

    let mut cmd = Command::new("jj");
    cmd.args(jj_args);
    cmd.args(["--config-file", &config_file.path().display().to_string()]);
    cmd.args(["--tool", "jj-hunk-tool"]);
    cmd.env(PATCH_ENV_VAR, patch_file.path());
    if reverse {
        cmd.env(REVERSE_ENV_VAR, "1");
    }

    let output = cmd.output().context("running jj")?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.is_empty() {
        eprint!("{stderr}");
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.is_empty() {
        print!("{stdout}");
    }

    if !output.status.success() {
        bail!("jj command failed");
    }

    Ok(())
}

/// Split selected hunks out of a revision using jj split --tool.
pub fn split_hunks(
    specs: &[HunkSpec<'_>],
    revision: Option<&str>,
    message: Option<&str>,
    parallel: bool,
    extra_args: &[&str],
) -> Result<()> {
    let patch_content = build_combined_patch(specs, false)?;
    if patch_content.is_empty() {
        bail!("no hunks selected");
    }

    let mut args: Vec<&str> = vec!["split"];
    if let Some(rev) = revision {
        args.extend_from_slice(&["-r", rev]);
    }
    let msg_storage;
    if let Some(msg) = message {
        msg_storage = msg.to_string();
        args.extend_from_slice(&["-m", &msg_storage]);
    }
    if parallel {
        args.push("--parallel");
    }
    args.extend_from_slice(extra_args);

    run_jj_with_tool(&args, &patch_content, false)?;
    Ok(())
}

/// Squash selected hunks from source into destination using jj squash --tool.
pub fn squash_hunks(specs: &[HunkSpec<'_>], extra_args: &[&str]) -> Result<()> {
    let patch_content = build_combined_patch(specs, false)?;
    if patch_content.is_empty() {
        bail!("no hunks selected");
    }
    let mut args = vec!["squash"];
    args.extend_from_slice(extra_args);
    run_jj_with_tool(&args, &patch_content, false)
}

/// Rewrite a revision in-place, keeping only the selected hunks.
pub fn diffedit_hunks(specs: &[HunkSpec<'_>], jj_extra_args: &[&str]) -> Result<()> {
    let patch_content = build_combined_patch(specs, false)?;
    if patch_content.is_empty() {
        bail!("no hunks selected");
    }
    let mut args = vec!["diffedit"];
    args.extend_from_slice(jj_extra_args);
    run_jj_with_tool(&args, &patch_content, false)
}

/// Restore (undo) selected hunks. The caller provides the jj-specific args
/// (e.g. ["--changes-in", "@"] or ["--from", "x", "--into", "y"]).
pub fn restore_hunks(specs: &[HunkSpec<'_>], jj_extra_args: &[&str]) -> Result<()> {
    let patch_content = build_combined_patch(specs, false)?;
    if patch_content.is_empty() {
        bail!("no hunks selected");
    }
    let mut args = vec!["restore"];
    args.extend_from_slice(jj_extra_args);
    run_jj_with_tool(&args, &patch_content, true)
}

/// A hunk fingerprint for stable matching across re-computations.
/// Uses file path + non-context lines (strips context which can shift).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HunkFingerprint {
    pub file: String,
    pub change_lines: Vec<String>,
}

impl HunkFingerprint {
    pub fn from_hunk(hunk: &DiffHunk) -> Self {
        let change_lines = hunk
            .lines
            .iter()
            .filter(|l| l.starts_with('+') || l.starts_with('-'))
            .cloned()
            .collect();
        HunkFingerprint {
            file: hunk.file.clone(),
            change_lines,
        }
    }
}

/// Result of routing a single hunk.
#[derive(Debug)]
pub struct HunkRouting {
    pub hunk_id: String,
    pub file: String,
    pub additions: usize,
    pub deletions: usize,
    pub target: Option<String>,
    pub candidates: Vec<String>,
    pub reason: &'static str,
}

/// Absorb hunks into ancestor commits based on annotation overlap.
pub fn absorb_hunks(
    selected: &[&(String, &DiffHunk)],
    source: &str,
    dry_run: bool,
    interactive: bool,
) -> Result<()> {
    use crate::diff;

    // 1. Get mutable ancestors
    let ancestors = diff::get_mutable_ancestors(source)?;
    if ancestors.is_empty() {
        println!("Nothing to absorb: no mutable ancestors.");
        return Ok(());
    }

    // 2. Compute annotations for each changed file
    let parent_rev = format!("{source}-");
    let mut annotations_cache: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();

    // 3. Route each hunk
    let mut routings: Vec<(HunkRouting, HunkFingerprint)> = Vec::new();

    for (id, hunk) in selected {
        let additions = hunk.lines.iter().filter(|l| l.starts_with('+')).count();
        let deletions = hunk.lines.iter().filter(|l| l.starts_with('-')).count();
        let fingerprint = HunkFingerprint::from_hunk(hunk);

        // New files can't be annotated
        if hunk.old_file == "/dev/null" {
            routings.push((
                HunkRouting {
                    hunk_id: id.clone(),
                    file: hunk.file.clone(),
                    additions,
                    deletions,
                    target: None,
                    candidates: vec![],
                    reason: "new file",
                },
                fingerprint,
            ));
            continue;
        }

        // Get annotations for this file (cached)
        let annotations = if let Some(cached) = annotations_cache.get(&hunk.file) {
            cached.clone()
        } else {
            match diff::get_jj_annotations(&parent_rev, &hunk.file) {
                Ok(ann) => {
                    annotations_cache.insert(hunk.file.clone(), ann.clone());
                    ann
                }
                Err(_) => {
                    routings.push((
                        HunkRouting {
                            hunk_id: id.clone(),
                            file: hunk.file.clone(),
                            additions,
                            deletions,
                            target: None,
                            candidates: vec![],
                            reason: "annotation failed",
                        },
                        fingerprint,
                    ));
                    continue;
                }
            }
        };

        // Parse the @@ header to get old-side line range
        let old_range = parse_old_range(&hunk.header);

        // Collect mutable ancestor change IDs from the hunk's changed lines.
        // Walk the hunk lines, track old-file line numbers, and only check
        // annotations for deleted/modified lines (prefix '-') and adjacent
        // context for pure insertions.
        let mut ancestor_hits: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();

        if let Some((old_start, _old_count)) = old_range {
            let has_deletions = hunk.lines.iter().any(|l| l.starts_with('-'));
            if has_deletions {
                // Track which old-file lines are deleted/modified
                let mut old_line = old_start; // 1-based
                for line in &hunk.lines {
                    if line.starts_with('-') {
                        // This old-file line is being removed/changed
                        if let Some(change_id) = annotations.get(old_line.saturating_sub(1)) {
                            if ancestors.contains(change_id) {
                                *ancestor_hits.entry(change_id.clone()).or_insert(0) += 1;
                            }
                        }
                        old_line += 1;
                    } else if line.starts_with('+') {
                        // Addition: doesn't consume an old line
                    } else {
                        // Context line: consumes an old line but don't count it
                        old_line += 1;
                    }
                }
            }
            // Pure insertions (no '-' lines): leave as unmatched.
            // There are no deleted/modified lines to blame, so we can't
            // determine which ancestor "owns" this region.
        }

        let (target, candidates, reason) = if ancestor_hits.len() == 1 {
            let target = ancestor_hits.into_keys().next().unwrap();
            (Some(target), vec![], "matched")
        } else if ancestor_hits.is_empty() {
            // Fallback: find the most recent mutable ancestor that touched this file
            if hunk.old_file != "/dev/null" {
                match diff::get_ancestors_touching_file(source, &hunk.file) {
                    Ok(file_ancestors) if !file_ancestors.is_empty() => {
                        let target = file_ancestors[0].clone();
                        (Some(target), vec![], "matched (file)")
                    }
                    _ => (None, vec![], "no overlapping ancestor hunk"),
                }
            } else {
                (None, vec![], "no overlapping ancestor hunk")
            }
        } else {
            let candidates: Vec<String> = ancestor_hits.into_keys().collect();
            (None, candidates, "ambiguous")
        };

        routings.push((
            HunkRouting {
                hunk_id: id.clone(),
                file: hunk.file.clone(),
                additions,
                deletions,
                target,
                candidates,
                reason,
            },
            fingerprint,
        ));
    }

    // 3b. Interactive review: let user accept/skip/retarget each hunk
    if interactive {
        let ancestor_list: Vec<String> = ancestors.iter().cloned().collect();
        let stdin = std::io::stdin();
        let mut quit = false;

        for (routing, _fp) in routings.iter_mut() {
            if quit {
                // After quit, skip remaining hunks (leave targets as-is won't matter,
                // but we need to clear target so they don't get absorbed)
                routing.target = None;
                routing.reason = "skipped (quit)";
                continue;
            }

            // Find the original hunk to display its content
            let hunk_opt = selected
                .iter()
                .find(|(id, _)| *id == routing.hunk_id)
                .map(|(_, h)| *h);

            // Display hunk with syntax highlighting and absolute line numbers
            let is_tty = console::Term::stdout().is_term();
            let header_style = if is_tty { Style::new().bold() } else { Style::new() };
            println!(
                "\n{} {} (+{} -{})",
                header_style.apply_to(&routing.hunk_id),
                header_style.apply_to(&routing.file),
                routing.additions,
                routing.deletions,
            );
            if let Some(hunk) = hunk_opt {
                print!("{}", format_hunk_display(hunk, is_tty));
            }

            // Show current target
            let target_desc = if let Some(ref t) = routing.target {
                let desc = diff::get_change_description(t).unwrap_or_default();
                if desc.is_empty() {
                    format!("Target: {t}")
                } else {
                    format!("Target: {t} ({desc})")
                }
            } else if routing.reason == "ambiguous" {
                let descs: Vec<String> = routing
                    .candidates
                    .iter()
                    .map(|c| {
                        let desc = diff::get_change_description(c).unwrap_or_default();
                        if desc.is_empty() {
                            c.clone()
                        } else {
                            format!("{c} ({desc})")
                        }
                    })
                    .collect();
                format!("Ambiguous: {}", descs.join(", "))
            } else {
                format!("Unmatched: {}", routing.reason)
            };
            println!("{target_desc}");

            // Prompt loop
            loop {
                print!("[a]bsorb / [s]kip / [t]arget / [q]uit: ");
                std::io::Write::flush(&mut std::io::stdout())?;
                let mut input = String::new();
                stdin.read_line(&mut input)?;
                let action = input.trim().to_lowercase();

                match action.as_str() {
                    "a" | "absorb" => {
                        if routing.target.is_none() {
                            println!("No target set. Use [t] to pick a target first.");
                            continue;
                        }
                        break;
                    }
                    "s" | "skip" => {
                        routing.target = None;
                        routing.reason = "skipped";
                        break;
                    }
                    "t" | "target" => {
                        // Show numbered list of ancestors
                        println!("Select target:");
                        for (i, cid) in ancestor_list.iter().enumerate() {
                            let desc = diff::get_change_description(cid).unwrap_or_default();
                            if desc.is_empty() {
                                println!("  {}: {cid}", i + 1);
                            } else {
                                println!("  {}: {cid} ({desc})", i + 1);
                            }
                        }
                        print!("Enter number: ");
                        std::io::Write::flush(&mut std::io::stdout())?;
                        let mut num_input = String::new();
                        stdin.read_line(&mut num_input)?;
                        if let Ok(n) = num_input.trim().parse::<usize>() {
                            if n >= 1 && n <= ancestor_list.len() {
                                routing.target = Some(ancestor_list[n - 1].clone());
                                routing.reason = "retargeted";
                                let desc =
                                    diff::get_change_description(&ancestor_list[n - 1])
                                        .unwrap_or_default();
                                println!("→ Retargeted to {}{}", ancestor_list[n - 1],
                                    if desc.is_empty() { String::new() } else { format!(" ({desc})") });
                                break;
                            }
                        }
                        println!("Invalid selection.");
                        continue;
                    }
                    "q" | "quit" => {
                        quit = true;
                        routing.target = None;
                        routing.reason = "skipped (quit)";
                        break;
                    }
                    _ => {
                        println!("Unknown action. Use a/s/t/q.");
                        continue;
                    }
                }
            }
        }
    }

    // 4. Print routing plan
    let absorbed: Vec<&(HunkRouting, HunkFingerprint)> =
        routings.iter().filter(|(r, _)| r.target.is_some()).collect();
    let ambiguous: Vec<&(HunkRouting, HunkFingerprint)> = routings
        .iter()
        .filter(|(r, _)| r.reason == "ambiguous")
        .collect();
    let unmatched: Vec<&(HunkRouting, HunkFingerprint)> = routings
        .iter()
        .filter(|(r, _)| r.target.is_none() && r.reason != "ambiguous")
        .collect();

    if absorbed.is_empty() {
        println!("Nothing to absorb: no hunks matched any ancestor.");
        if !ambiguous.is_empty() {
            println!("\nAmbiguous (staying in {source}):");
            for (r, _) in &ambiguous {
                let descs: Vec<String> = r
                    .candidates
                    .iter()
                    .map(|c| {
                        let desc = diff::get_change_description(c).unwrap_or_default();
                        if desc.is_empty() {
                            c.clone()
                        } else {
                            format!("{c} ({desc})")
                        }
                    })
                    .collect();
                println!(
                    "  {} ({} +{} -{}) — overlaps {}",
                    r.hunk_id,
                    r.file,
                    r.additions,
                    r.deletions,
                    descs.join(", ")
                );
            }
        }
        if !unmatched.is_empty() {
            println!("\nUnmatched (staying in {source}):");
            for (r, _) in &unmatched {
                println!(
                    "  {} ({} +{} -{}) — {}",
                    r.hunk_id, r.file, r.additions, r.deletions, r.reason
                );
            }
        }
        return Ok(());
    }

    let verb = if dry_run { "Would absorb" } else { "Absorbed" };
    println!("{verb} {} hunk(s):", absorbed.len());
    for (r, _) in &absorbed {
        let target = r.target.as_ref().unwrap();
        let desc = diff::get_change_description(target).unwrap_or_default();
        let desc_part = if desc.is_empty() {
            String::new()
        } else {
            format!(" ({desc})")
        };
        println!(
            "  {} ({} +{} -{}) → {target}{desc_part}",
            r.hunk_id, r.file, r.additions, r.deletions
        );
    }
    if !ambiguous.is_empty() {
        println!("\nAmbiguous (staying in {source}):");
        for (r, _) in &ambiguous {
            let descs: Vec<String> = r
                .candidates
                .iter()
                .map(|c| {
                    let desc = diff::get_change_description(c).unwrap_or_default();
                    if desc.is_empty() {
                        c.clone()
                    } else {
                        format!("{c} ({desc})")
                    }
                })
                .collect();
            println!(
                "  {} ({} +{} -{}) — overlaps {}",
                r.hunk_id,
                r.file,
                r.additions,
                r.deletions,
                descs.join(", ")
            );
        }
    }
    if !unmatched.is_empty() {
        println!("\nUnmatched (staying in {source}):");
        for (r, _) in &unmatched {
            println!(
                "  {} ({} +{} -{}) — {}",
                r.hunk_id, r.file, r.additions, r.deletions, r.reason
            );
        }
    }

    if dry_run {
        return Ok(());
    }

    // 5. Execute: sequential squash per target, re-identifying by fingerprint
    // Group absorbed hunks by target
    let mut target_groups: std::collections::HashMap<String, Vec<HunkFingerprint>> =
        std::collections::HashMap::new();
    for (r, fp) in &routings {
        if let Some(ref target) = r.target {
            target_groups
                .entry(target.clone())
                .or_default()
                .push(fp.clone());
        }
    }

    for (target, fingerprints) in &target_groups {
        // Re-get current diff (it changes after each squash)
        let raw = diff::get_jj_diff(&Some(source.to_string()))?;
        let hunks = crate::diff::parse_diff(&raw);
        let identified = crate::diff::assign_ids(&hunks);

        // Match current hunks to fingerprints
        let mut specs: Vec<HunkSpec<'_>> = Vec::new();
        for (hid, hunk) in &identified {
            let fp = HunkFingerprint::from_hunk(hunk);
            if fingerprints.contains(&fp) {
                specs.push((hid.as_str(), *hunk, vec![]));
            }
        }

        if specs.is_empty() {
            continue;
        }

        let patch_content = build_combined_patch(&specs, false)?;
        if patch_content.is_empty() {
            continue;
        }

        let args: Vec<&str> = vec!["squash", "--from", source, "--into", target];
        run_jj_with_tool(&args, &patch_content, false)?;
    }

    Ok(())
}

/// Parse the old-side range from a @@ header.
/// Returns (start_line, count) from "@@ -start,count ..."
fn parse_old_range(header: &str) -> Option<(usize, usize)> {
    // Format: "@@ -start,count +start,count @@" or "@@ -start +start,count @@"
    let header = header.trim();
    let after_at = header.strip_prefix("@@ -")?;
    let end = after_at.find(' ')?;
    let range_str = &after_at[..end];
    if let Some((start_s, count_s)) = range_str.split_once(',') {
        let start: usize = start_s.parse().ok()?;
        let count: usize = count_s.parse().ok()?;
        Some((start, count))
    } else {
        let start: usize = range_str.parse().ok()?;
        Some((start, 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_old_range_with_count() {
        assert_eq!(
            parse_old_range("@@ -1,3 +1,3 @@"),
            Some((1, 3))
        );
    }

    #[test]
    fn parse_old_range_without_count() {
        assert_eq!(
            parse_old_range("@@ -5 +5,2 @@"),
            Some((5, 1))
        );
    }

    #[test]
    fn parse_old_range_zero_count() {
        assert_eq!(
            parse_old_range("@@ -10,0 +10,3 @@"),
            Some((10, 0))
        );
    }

    #[test]
    fn parse_old_range_with_context() {
        assert_eq!(
            parse_old_range("@@ -1,3 +1,3 @@ fn main()"),
            Some((1, 3))
        );
    }

    #[test]
    fn parse_old_range_invalid() {
        assert_eq!(parse_old_range("not a header"), None);
    }

    #[test]
    fn parse_header_ranges_both() {
        assert_eq!(parse_header_ranges("@@ -7,3 +7,4 @@"), (7, 7));
    }

    #[test]
    fn parse_header_ranges_different_starts() {
        assert_eq!(parse_header_ranges("@@ -10,5 +12,3 @@"), (10, 12));
    }

    #[test]
    fn parse_header_ranges_with_context_label() {
        assert_eq!(
            parse_header_ranges("@@ -1,3 +1,3 @@ fn main()"),
            (1, 1)
        );
    }

    #[test]
    fn parse_header_ranges_no_count() {
        assert_eq!(parse_header_ranges("@@ -5 +5,2 @@"), (5, 5));
    }

    #[test]
    fn format_hunk_display_absolute_lines() {
        let hunk = DiffHunk {
            file: "test.rs".into(),
            old_file: "test.rs".into(),
            new_file: "test.rs".into(),
            file_header: String::new(),
            header: "@@ -7,3 +7,3 @@".into(),
            lines: vec![
                " context".into(),
                "-old_line".into(),
                "+new_line".into(),
                " context2".into(),
            ],
            unsupported_metadata: None,
        };
        let output = format_hunk_display(&hunk, false);
        // Should contain absolute line numbers starting at 7
        assert!(output.contains(" 7:"), "should start at line 7: {output}");
        assert!(output.contains(" 8:"), "should have line 8: {output}");
        assert!(!output.contains(" 1:"), "should NOT have line 1: {output}");
    }

    #[test]
    fn fingerprint_ignores_context() {
        let hunk1 = DiffHunk {
            file: "a.txt".into(),
            old_file: "a.txt".into(),
            new_file: "a.txt".into(),
            file_header: String::new(),
            header: String::new(),
            lines: vec![
                " context1".into(),
                "-old".into(),
                "+new".into(),
                " context2".into(),
            ],
            unsupported_metadata: None,
        };
        let hunk2 = DiffHunk {
            file: "a.txt".into(),
            old_file: "a.txt".into(),
            new_file: "a.txt".into(),
            file_header: String::new(),
            header: String::new(),
            lines: vec![
                " different_context".into(),
                "-old".into(),
                "+new".into(),
                " also_different".into(),
            ],
            unsupported_metadata: None,
        };
        assert_eq!(
            HunkFingerprint::from_hunk(&hunk1),
            HunkFingerprint::from_hunk(&hunk2),
        );
    }
}

/// JJ tool protocol handler.
///
/// JJ invokes: `jj-hunk-tool _jj-tool $left $right`
/// - `$left` = parent/base state directory (read-only)
/// - `$right` = current state directory (writable)
///
/// Algorithm:
/// 1. Read patch path from JJ_HUNK_TOOL_PATCH env var
/// 2. Reset $right to match $left (copy all files from left, remove extras)
/// 3. Apply the patch to $right
pub fn jj_tool_apply(left: &str, right: &str) -> Result<()> {
    let patch_path = std::env::var(PATCH_ENV_VAR)
        .with_context(|| format!("{PATCH_ENV_VAR} environment variable not set"))?;

    let left_path = Path::new(left);
    let right_path = Path::new(right);

    // Step 1: Reset $right to $left state
    reset_dir_to(left_path, right_path)?;

    // Step 2: Apply the pre-computed patch
    let reverse = std::env::var(REVERSE_ENV_VAR).is_ok();
    let mut patch_cmd = Command::new("patch");
    patch_cmd.args(["-p1", "--silent"]);
    if reverse {
        patch_cmd.arg("--reverse");
    }
    patch_cmd.arg("-i").arg(&patch_path);
    patch_cmd.current_dir(right_path);
    let status = patch_cmd.status().context("failed to run patch")?;

    if !status.success() {
        bail!("patch failed to apply (exit code: {:?})", status.code());
    }

    Ok(())
}

/// Reset `dst` directory to match `src` directory contents.
fn reset_dir_to(src: &Path, dst: &Path) -> Result<()> {
    remove_dir_contents(dst)?;
    copy_dir_recursive(src, dst)?;
    Ok(())
}

fn remove_dir_contents(dir: &Path) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading dir {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            std::fs::remove_dir_all(&path)
                .with_context(|| format!("removing dir {}", path.display()))?;
        } else {
            std::fs::remove_file(&path)
                .with_context(|| format!("removing file {}", path.display()))?;
        }
    }
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    for entry in std::fs::read_dir(src).with_context(|| format!("reading dir {}", src.display()))? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            std::fs::create_dir_all(&dst_path)
                .with_context(|| format!("creating dir {}", dst_path.display()))?;
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path).with_context(|| {
                format!("copying {} to {}", src_path.display(), dst_path.display())
            })?;
        }
    }
    Ok(())
}
