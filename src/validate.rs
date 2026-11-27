//! Generator-side validation of the patch payload.
//!
//! The tool used to hand its payload to GNU patch, which is the stricter of the
//! two appliers about the *shape* of a hunk and the more fragile one in practice
//! (interactive prompts on unusual paths, `.orig`/`.rej` leftovers, fuzz
//! heuristics). It now hands the payload to plain `git apply`, which is atomic and
//! creates no backup files, but is much less strict about the *shape* of a hunk:
//! it locates a hunk by content, so it applies one whose context is unbalanced,
//! whose context is longer than `jj` ever emits, or that carries no context at all
//! where it happens to land. That accidental strictness is what this module
//! replaces.
//!
//! The arithmetic rules are not replacements — `git apply` rejects a header whose
//! counts do not describe the body, and a GNU patch that silently applies it is
//! the loose one there. They are kept because the tool can say *which hunk* is
//! wrong, and say it before `_jj-tool` has reset the target directory, instead of
//! surfacing as git's "corrupt patch at line 8" midway through an apply.
//!
//! [`validate_patch`] is a pure function over the payload text. It runs on every
//! payload the builder produces, before any applier sees it, so a generator bug
//! fails loudly, naming the offending hunk, instead of applying a patch that no
//! other tool could read. It is also run by the `_jj-tool` protocol, where the
//! payload arrives through the environment.
//!
//! # The rules
//!
//! * **At least one change** — every `@@` block must change something. A block
//!   whose lines are all context is what a line-range selection that covers no
//!   change line slices down to, and neither applier will read it: `git apply`
//!   says "corrupt patch", GNU patch says "malformed patch". A selection that
//!   changes nothing is an error here, not a payload.
//! * **Header arithmetic** — the `@@` counts must describe the body: a context
//!   line is one old and one new line, a `-` line one old line, a `+` line one new
//!   line. A side with a non-zero count must start at line 1 or later; only an
//!   empty side (`-0,0`) may be numbered 0, meaning "before the first line".
//! * **Context ceiling** — at most [`CONTEXT_CEILING`] context lines on each side
//!   of the changes. `jj` emits three and the builder only ever removes context,
//!   so more than three is a generator bug (or a hand-edited payload). This is a
//!   ceiling, not a clamp: nothing trims surplus context, because surplus context
//!   cannot come from the generator in the first place.
//! * **Context balance** — the number of context lines before and after the
//!   changes must be within one line of each other. GNU patch reads unbalanced
//!   context as "this hunk is anchored at the start or end of the file" and
//!   rejects it anywhere else (issue #2); the builder balances the longer side
//!   down to the shorter one and keeps at least one line, so the only shapes it
//!   emits are `n`/`n` (0 ≤ n ≤ 3) and `0`/`1` or `1`/`0` at a file boundary.
//!   Anything further apart is a generator bug even when an applier happens to
//!   fuzz it through.
//! * **No context at all beside context** — a hunk with no context lines is only
//!   legitimate when the patch has no context for that file either: the whole
//!   file is rewritten, or the file is new, deleted or a single line. If other
//!   hunks for the same file carry context, a context-free hunk means the
//!   generator dropped the context it should have kept, which is exactly the
//!   shape `git apply` refuses to place (it will not locate a hunk without
//!   context in the middle of a file) and GNU patch fuzzes through. A whole-file
//!   hunk is never accompanied by other hunks for that file, so the rule cannot
//!   reject a shape the builder legitimately emits.
//!
//! The `\ No newline at end of file` marker is not a context line: it annotates
//! the line before it and counts towards neither side.
//!
//! See the "Patch application" section of the README for the runtime split (git
//! apply in the tool, GNU patch as a test-only oracle) and why it exists.

use anyhow::{Result, bail, format_err};

use crate::markers::{self, Marker};

/// The most context lines the payload may carry on one side of the changes.
///
/// `jj diff --git` emits three, and the payload builder never adds context, so a
/// payload above this has been assembled from something other than a jj hunk.
/// This is a validator ceiling, not a clamp: see the module docs.
pub const CONTEXT_CEILING: usize = 3;

/// Validate a generated patch payload.
///
/// Returns `Ok(())` for an empty payload — "nothing selected" is the caller's
/// question to answer (the apply modes reject it themselves), not a malformed
/// patch. Every other payload must be a well-formed, balanced unified diff.
pub fn validate_patch(patch: &str) -> Result<()> {
    let hunks = parse_patch(patch)?;
    for (index, hunk) in hunks.iter().enumerate() {
        check_context(index, hunk, &hunks)?;
    }
    markers::hit(Marker::ValidateAccepted);
    Ok(())
}

/// One `@@` block of the payload, as much of it as validation needs.
#[derive(Debug)]
struct Hunk<'a> {
    /// The path the hunk changes, as a stable key for per-file rules: the new
    /// side, falling back to the old side for a deletion.
    file: String,
    /// The `@@ … @@` line, verbatim, for error messages.
    header: &'a str,
    /// Context lines before the first change.
    leading: usize,
    /// Context lines after the last change.
    trailing: usize,
    /// Lines that change something (`-` and `+`), context excluded.
    changes: usize,
}

impl Hunk<'_> {
    fn context(&self) -> usize {
        self.leading + self.trailing
    }

    /// How the hunk is named in an error: payload position, file, header.
    fn label(&self, index: usize) -> String {
        format!(
            "payload hunk #{} for {:?} ({})",
            index + 1,
            self.file,
            self.header
        )
    }
}

/// The `@@` ranges of one hunk.
#[derive(Debug, PartialEq, Eq)]
struct Ranges {
    old_start: usize,
    old_count: usize,
    new_start: usize,
    new_count: usize,
}

impl Ranges {
    fn new(old_start: usize, old_count: usize, new_start: usize, new_count: usize) -> Result<Self> {
        // A side with lines starts at line 1 or later. Only an empty side may be
        // numbered 0 ("the line before the first"), which is how a unified diff
        // spells an insertion at the top of a file.
        for (side, start, count) in [("old", old_start, old_count), ("new", new_start, new_count)] {
            if count > 0 && start == 0 {
                return Err(format_err!(
                    "{side} side of the hunk header starts at line 0 but claims {count} line(s); \
                     only an empty side may be numbered 0"
                ));
            }
        }
        Ok(Self {
            old_start,
            old_count,
            new_start,
            new_count,
        })
    }
}

/// Parse every hunk in `patch`, validating each `@@` header against its body.
fn parse_patch(patch: &str) -> Result<Vec<Hunk<'_>>> {
    let mut hunks: Vec<Hunk<'_>> = Vec::new();
    let mut old_file = String::new();
    let mut new_file = String::new();
    let mut lines = patch.lines().peekable();

    while let Some(line) = lines.next() {
        // `--- ` and `+++ ` only appear as a section header, never inside a body:
        // the body of a hunk is consumed by its counts (and a run of added lines
        // may well spell `+++ b/x`), so reaching one here means the previous hunk
        // was complete.
        if let Some(rest) = line.strip_prefix("--- ") {
            old_file = diff_path(rest);
            continue;
        }
        if let Some(rest) = line.strip_prefix("+++ ") {
            new_file = diff_path(rest);
            continue;
        }
        if line.is_empty() {
            markers::hit(Marker::ErrorPayloadShape);
            return Err(format_err!(
                "empty line in the patch payload, outside any hunk body"
            ));
        }

        let ranges = parse_header(line).inspect_err(|_| markers::hit(Marker::ErrorPayloadShape))?;
        let file = if new_file.is_empty() || new_file.ends_with("dev/null") {
            old_file.clone()
        } else {
            new_file.clone()
        };
        let index = hunks.len();
        let label = format!("payload hunk #{} for {file:?} ({line})", index + 1);

        let mut old_seen = 0usize;
        let mut new_seen = 0usize;
        let mut leading = 0usize;
        let mut trailing = 0usize;
        let mut saw_change = false;
        let mut changes = 0usize;
        let mut after_body_line = false;

        while let Some(next) = lines.peek().copied() {
            // "\ No newline at end of file" annotates the line before it and
            // counts towards neither side of the hunk.
            if next.starts_with('\\') {
                if !after_body_line {
                    markers::hit(Marker::ErrorPayloadNoNewlineMarker);
                    bail!(
                        "{label}: a `\\ No newline` marker does not follow a hunk line; it \
                         annotates the line before it and stands for neither side"
                    );
                }
                lines.next();
                after_body_line = false;
                continue;
            }
            // A `--- ` line followed by a `+++ ` line is a new file section, not
            // a removal of a line whose text begins with `-- `. The payload is
            // one section per selected hunk, so the same file appears again for
            // every hunk; git's own parser disambiguates the same way.
            if next.starts_with("--- ") && next_is_section_header(&lines) {
                break;
            }
            let sign = next.chars().next();
            if !matches!(sign, Some(' ') | Some('+') | Some('-')) {
                break;
            }
            if old_seen == ranges.old_count && new_seen == ranges.new_count {
                markers::hit(Marker::ErrorPayloadShape);
                bail!(
                    "{label}: the body carries more lines than the header's \
                     {}/{} old/new counts describe",
                    ranges.old_count,
                    ranges.new_count
                );
            }
            lines.next();
            match sign {
                Some(' ') => {
                    if saw_change {
                        trailing += 1;
                    } else {
                        leading += 1;
                    }
                    old_seen += 1;
                    new_seen += 1;
                }
                Some('-') => {
                    saw_change = true;
                    changes += 1;
                    trailing = 0;
                    old_seen += 1;
                }
                _ => {
                    saw_change = true;
                    changes += 1;
                    trailing = 0;
                    new_seen += 1;
                }
            }
            after_body_line = true;
        }

        if old_seen != ranges.old_count || new_seen != ranges.new_count {
            markers::hit(Marker::ErrorPayloadShape);
            bail!(
                "{label}: the header claims {} old / {} new line(s) but the body has \
                 {} old / {} new",
                ranges.old_count,
                ranges.new_count,
                old_seen,
                new_seen
            );
        }

        hunks.push(Hunk {
            file,
            header: line,
            leading,
            trailing,
            changes,
        });
    }

    Ok(hunks)
}

/// Whether the line after the next one starts a file section, i.e. whether the
/// next line is a `--- ` header rather than a removal beginning with `-- `.
fn next_is_section_header<'a, I>(lines: &std::iter::Peekable<I>) -> bool
where
    I: Iterator<Item = &'a str> + Clone,
{
    let mut probe = lines.clone();
    probe.next();
    probe
        .next()
        .is_some_and(|following| following.starts_with("+++ "))
}

/// The changed path from a `--- a/x` / `+++ b/x` line, without its `a/` or `b/`
/// prefix. `/dev/null` is kept as the marker it is.
fn diff_path(side: &str) -> String {
    let side = side.trim_end();
    side.strip_prefix("a/")
        .or_else(|| side.strip_prefix("b/"))
        .unwrap_or(side)
        .to_string()
}

/// Parse `@@ -start[,count] +start[,count] @@ optional function context`.
fn parse_header(header: &str) -> Result<Ranges> {
    let rest = header
        .strip_prefix("@@ ")
        .ok_or_else(|| format_err!("hunk header does not start with `@@ `: {header:?}"))?;
    let (ranges, _) = rest.split_once(" @@").ok_or_else(|| {
        format_err!("hunk header is not terminated by ` @@` (and, per the hunk ranges): {header:?}")
    })?;
    let mut sides = ranges.split(' ');
    let old = sides
        .next()
        .ok_or_else(|| format_err!("hunk header has no old range: {header:?}"))?;
    let new = sides
        .next()
        .ok_or_else(|| format_err!("hunk header has no new range: {header:?}"))?;
    if sides.next().is_some() {
        return Err(format_err!(
            "hunk header has trailing content between its ranges and ` @@`: {header:?}"
        ));
    }
    let (old_start, old_count) = parse_range(old, '-', header)?;
    let (new_start, new_count) = parse_range(new, '+', header)?;
    Ranges::new(old_start, old_count, new_start, new_count)
        .map_err(|error| format_err!("{error} (header: {header:?})"))
}

fn parse_range(range: &str, sign: char, header: &str) -> Result<(usize, usize)> {
    let body = range.strip_prefix(sign).ok_or_else(|| {
        format_err!("hunk header range {range:?} does not start with {sign:?} (header: {header:?})")
    })?;
    let (start, count) = match body.split_once(',') {
        Some((start, count)) => (start, Some(count)),
        None => (body, None),
    };
    let start: usize = start
        .parse()
        .map_err(|_| format_err!("hunk header range {range:?} has an unparsable start"))?;
    let count = match count {
        Some(count) => count
            .parse()
            .map_err(|_| format_err!("hunk header range {range:?} has an unparsable count"))?,
        // A range without a count is exactly one line.
        None => 1,
    };
    Ok((start, count))
}

/// The per-hunk rules: at least one change, ceiling, balance, and no context
/// beside context.
fn check_context(index: usize, hunk: &Hunk<'_>, hunks: &[Hunk<'_>]) -> Result<()> {
    let label = hunk.label(index);

    // A block of nothing but context changes nothing. It is what a line-range
    // selection that covers no change line slices to (`slice_hunk` neutralises
    // an excluded removal into context), and both appliers reject it as a
    // malformed patch, so it must not reach either one.
    if hunk.changes == 0 {
        markers::hit(Marker::ErrorPayloadNoChanges);
        bail!(
            "{label}: no changes in the hunk — every line is context. Neither \
             `git apply` nor GNU patch accepts an all-context hunk, so the selected \
             line range covers no change line"
        );
    }

    if hunk.leading > CONTEXT_CEILING {
        markers::hit(Marker::ErrorPayloadContextCeiling);
        bail!(
            "{label}: {} context line(s) before the changes, but `jj` emits at most \
             {CONTEXT_CEILING} and the builder never adds any",
            hunk.leading
        );
    }
    if hunk.trailing > CONTEXT_CEILING {
        markers::hit(Marker::ErrorPayloadContextCeiling);
        bail!(
            "{label}: {} context line(s) after the changes, but `jj` emits at most \
             {CONTEXT_CEILING} and the builder never adds any",
            hunk.trailing
        );
    }

    if hunk.leading.abs_diff(hunk.trailing) > 1 {
        markers::hit(Marker::ErrorPayloadContextImbalance);
        bail!(
            "{label}: {} context line(s) before the changes and {} after. GNU patch \
             reads unbalanced context as \"this hunk sits at the start or end of the \
             file\" and rejects it anywhere else (issue #2); the builder balances the \
             longer side down to the shorter one, keeping one line at a file boundary, \
             so the two sides are never more than one line apart",
            hunk.leading,
            hunk.trailing
        );
    }

    if hunk.context() == 0
        && let Some(other) = hunks.iter().enumerate().find(|(other_index, other)| {
            *other_index != index && other.file == hunk.file && other.context() > 0
        })
    {
        markers::hit(Marker::ErrorPayloadZeroContext);
        bail!(
            "{label}: no context lines at all, while {} carries {}. A hunk with no \
                 context is only sound when the whole file is rewritten or absent (a new \
                 file, a deleted file, a single-line file), where it is the file's only \
                 hunk; beside context it is context the generator dropped",
            other.1.label(other.0),
            plural(other.1.context(), "context line")
        );
    }

    Ok(())
}

/// `"1 context line"`, `"2 context lines"`.
fn plural(count: usize, singular: &str) -> String {
    if count == 1 {
        format!("{count} {singular}")
    } else {
        format!("{count} {singular}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markers::test_lock;

    /// A payload section for `f.txt` with the given `@@` header and body lines.
    fn payload(header: &str, body: &[&str]) -> String {
        let mut patch = String::from("--- a/f.txt\n+++ b/f.txt\n");
        patch.push_str(header);
        patch.push('\n');
        for line in body {
            patch.push_str(line);
            patch.push('\n');
        }
        patch
    }

    /// The rule-3 shape: header counts that do not describe the body. The header
    /// promises three lines on the new side, the body has two.
    #[test]
    fn rejects_header_counts_that_do_not_describe_the_body() {
        let _guard = test_lock();
        let patch = payload("@@ -2,3 +2,3 @@", &[" a", "-b", "+new"]);
        let error = validate_patch(&patch).expect_err("the counts are wrong");
        let error = error.to_string();
        assert!(
            error.contains("the header claims 3 old / 3 new") && error.contains("2 old / 2 new"),
            "the error must name both counts: {error}"
        );
        assert!(error.contains("payload hunk #1 for \"f.txt\""), "{error}");
        assert!(!markers::is_hit(Marker::ValidateAccepted));
    }

    /// The rule-3 shape in the other direction: a body line past the counts.
    #[test]
    fn rejects_a_body_longer_than_its_header_counts() {
        let _guard = test_lock();
        let patch = payload("@@ -1,2 +1,2 @@", &[" a", " b", " c"]);
        let error = validate_patch(&patch).expect_err("the body is longer than the header");
        assert!(
            error.to_string().contains("more lines than the header"),
            "{error}"
        );
    }

    /// An old side that claims lines while starting at line 0 is nonsense even
    /// if the counts happen to describe the body.
    #[test]
    fn rejects_a_non_empty_side_numbered_zero() {
        let _guard = test_lock();
        let patch = payload("@@ -0,3 +0,3 @@", &[" a", " b", " c"]);
        let error = validate_patch(&patch).expect_err("a non-empty side cannot start at 0");
        assert!(error.to_string().contains("starts at line 0"), "{error}");
    }

    /// A block of pure context changes nothing and is rejected before the
    /// context rules can even describe it. This is the shape a line range that
    /// covers no change line slices to.
    #[test]
    fn rejects_a_hunk_with_no_changes() {
        let _guard = test_lock();
        let patch = payload("@@ -2,3 +2,3 @@", &[" one", " two", " three"]);
        let error = validate_patch(&patch).expect_err("an all-context hunk changes nothing");
        let error = error.to_string();
        assert!(error.contains("no changes in the hunk"), "{error}");
        assert!(error.contains("payload hunk #1 for \"f.txt\""), "{error}");
        assert!(markers::is_hit(Marker::ErrorPayloadNoChanges));
    }

    /// Rule 2: more than [`CONTEXT_CEILING`] context lines on one side. `jj`
    /// never emits four, so this is the canary for a generator that starts
    /// synthesising context.
    #[test]
    fn rejects_more_than_three_context_lines_on_a_side() {
        let _guard = test_lock();
        // Four before the change, one after: within one line of each other, so
        // only the ceiling rejects it.
        let patch = payload(
            "@@ -1,6 +1,6 @@",
            &[" a", " b", " c", " d", "-e", "+E", " f"],
        );
        let error = validate_patch(&patch).expect_err("four leading context lines");
        let error = error.to_string();
        assert!(error.contains("4 context line(s) before"), "{error}");
        assert!(markers::is_hit(Marker::ErrorPayloadContextCeiling));
    }

    /// Rule 1: unbalanced context, the shape GNU patch misreads as a
    /// file-boundary hunk (issue #2).
    #[test]
    fn rejects_unbalanced_context() {
        let _guard = test_lock();
        let patch = payload("@@ -1,5 +1,5 @@", &[" a", " b", " c", "-d", "+D", " e"]);
        let error = validate_patch(&patch).expect_err("three before, one after");
        let error = error.to_string();
        assert!(
            error.contains("3 context line(s) before the changes and 1 after"),
            "{error}"
        );
        assert!(markers::is_hit(Marker::ErrorPayloadContextImbalance));
    }

    /// The one-line floor is *not* unbalanced: a boundary hunk keeps a single
    /// line of context and passes.
    #[test]
    fn accepts_the_one_line_floor_at_a_file_boundary() {
        let _guard = test_lock();
        let patch = payload("@@ -1,2 +1,2 @@", &["-a", "+A", " b"]);
        validate_patch(&patch).expect("0 before / 1 after is the boundary floor");
        let patch = payload("@@ -9,2 +9,2 @@", &[" a", "-b", "+B"]);
        validate_patch(&patch).expect("1 before / 0 after is the boundary floor");
    }

    /// Rule 4: a hunk with no context at all beside hunks that carry context.
    #[test]
    fn rejects_a_context_free_hunk_beside_context() {
        let _guard = test_lock();
        let mut patch = payload("@@ -2,3 +2,3 @@", &[" a", "-b", "+B", " c"]);
        // The same file again, with no context of its own.
        patch.push_str("--- a/f.txt\n+++ b/f.txt\n@@ -20,1 +20,1 @@\n-x\n+X\n");
        let error = validate_patch(&patch).expect_err("no context beside context");
        let error = error.to_string();
        assert!(
            error.contains("payload hunk #2 for \"f.txt\"")
                && error.contains("no context lines at all"),
            "{error}"
        );
        assert!(error.contains("payload hunk #1"), "{error}");
        assert!(markers::is_hit(Marker::ErrorPayloadZeroContext));
    }

    /// A hunk with no context is sound when the file has nothing to carry
    /// context from: a whole-file rewrite, and a new file.
    #[test]
    fn accepts_a_whole_file_hunk_without_context() {
        let _guard = test_lock();
        validate_patch(&payload("@@ -1,2 +1,2 @@", &["-a", "-b", "+A", "+B"]))
            .expect("a whole-file rewrite has no context to keep");
        validate_patch(&payload(
            "@@ -1,4 +1,4 @@",
            &["-a", " b", "-c", "+A", " b", "+C"],
        ))
        .expect("context only in the middle is still a whole-file hunk");

        let new_file = "--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1,2 @@\n+a\n+b\n";
        validate_patch(new_file).expect("a new file has no context");
        let deleted = "--- a/gone.txt\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-a\n-b\n";
        validate_patch(deleted).expect("a deleted file has no context");
    }

    /// Context on both sides, no newline markers: the ordinary shape.
    #[test]
    fn accepts_an_ordinary_balanced_hunk_across_files() {
        let _guard = test_lock();
        let mut patch = payload("@@ -10,3 +10,3 @@ fn f", &[" a", "-b", "+B", " c"]);
        patch.push_str("--- a/other.txt\n+++ b/other.txt\n@@ -1,2 +1,2 @@\n-a\n+A\n b\n");
        validate_patch(&patch).expect("two files, both balanced");
    }

    /// The no-newline marker annotates a line, counts towards neither side, and
    /// may follow either side's last line.
    #[test]
    fn counts_the_no_newline_marker_on_neither_side() {
        let _guard = test_lock();
        let patch = payload(
            "@@ -1,3 +1,3 @@",
            &[
                " a",
                "-old",
                "\\ No newline at end of file",
                "+new",
                "\\ No newline at end of file",
            ],
        );
        // Old side: 1 context + 1 removal = 2, not 3: the marker is not a line.
        let error = validate_patch(&patch).expect_err("the counts still describe the body");
        assert!(
            error.to_string().contains("claims 3 old / 3 new"),
            "{error}"
        );

        let corrected = payload(
            "@@ -1,2 +1,2 @@",
            &[
                " a",
                "-old",
                "\\ No newline at end of file",
                "+new",
                "\\ No newline at end of file",
            ],
        );
        validate_patch(&corrected).expect("markers annotate the lines before them");
    }

    /// A malformed header is rejected rather than skipped.
    #[test]
    fn rejects_an_unparsable_header() {
        let _guard = test_lock();
        for header in ["@@ -1,2 @@", "@@ -1,2 +1,2", "@@ nonsense @@"] {
            let error = validate_patch(&payload(header, &[" a"]))
                .expect_err("a malformed header must be rejected");
            assert!(
                error.to_string().contains("hunk header"),
                "{header}: {error}"
            );
        }
    }

    /// The empty payload is not malformed; the apply modes reject it separately.
    #[test]
    fn accepts_the_empty_payload() {
        let _guard = test_lock();
        validate_patch("").expect("nothing selected is not a malformed patch");
    }
}
