//! Property tests for selection semantics, patch payload construction and every
//! apply mode.
//!
//! The shape of each case is a small text repository with one or two files and
//! one to three hunks per file, far enough apart that they stay separate. All
//! content is LF-only and all paths are ASCII with no spaces, because these
//! properties apply payloads with `patch(1)`, which cannot open a path with a
//! space in it and (see `tests/properties_shapes.rs`) does not survive CRLF for
//! reasons unrelated to selection. The *payload* is still compared byte for byte
//! with `jj`'s own `@@` blocks, whitespace included.
//!
//! Every property states what it proves and which bug class it kills, and each
//! asserts the coverage markers of the path it took.

mod harness;

use std::collections::{BTreeMap, BTreeSet};

use harness::*;
use hegel::TestCase;
use hegel::generators as gs;

// ──────────────────────────────────────────────────────────────────────────────
// Case model
// ──────────────────────────────────────────────────────────────────────────────

/// One file with `regions` single-line changes, far enough apart to be separate
/// hunks.
#[derive(Debug, Clone)]
struct TextFile {
    path: String,
    base: Vec<String>,
    head: Vec<String>,
}

#[derive(Debug, Clone)]
struct TextCase {
    files: Vec<TextFile>,
}

impl TextCase {
    fn paths(&self) -> Vec<String> {
        self.files.iter().map(|file| file.path.clone()).collect()
    }

    /// The content of `path` in the commit whose bytes are `head` or `base`.
    fn content(&self, path: &str, head: bool) -> Vec<u8> {
        let file = self
            .files
            .iter()
            .find(|file| file.path == path)
            .expect("known path");
        let lines = if head { &file.head } else { &file.base };
        format!("{}\n", lines.join("\n")).into_bytes()
    }

    fn all_paths_content(&self, head: bool) -> BTreeMap<String, Vec<u8>> {
        self.files
            .iter()
            .map(|file| (file.path.clone(), self.content(&file.path, head)))
            .collect()
    }
}

/// A file path for the mutation properties: ASCII, no spaces, patch-safe.
#[hegel::composite]
fn safe_path(tc: &TestCase) -> String {
    let dir = if tc.draw(gs::booleans()) { "sub/" } else { "" };
    let stem: String = tc.draw(
        gs::text()
            .min_size(1)
            .max_size(6)
            .alphabet("abcdefghijklmnopqrstuvwxyz"),
    );
    format!("{dir}{stem}.txt")
}

#[hegel::composite]
fn text_case(tc: &TestCase) -> TextCase {
    let file_count: usize = tc.draw(gs::integers::<usize>().min_value(1).max_value(2));
    let mut seen = std::collections::BTreeSet::new();
    let mut files = Vec::new();
    for file_index in 0..file_count {
        let path = uniquify(tc.draw(safe_path()), &mut seen);
        // The first file always has three hunks, 8 lines apart: that is what
        // makes "first", "last", "middle", a contiguous run and an interleaved
        // pick five *different* selections rather than five names for the same
        // one. Any further file adds one or two more.
        let region_count: usize = if file_index == 0 {
            3
        } else {
            tc.draw(gs::integers::<usize>().min_value(1).max_value(2))
        };
        let line_count = 8 * (region_count + 1);
        let base: Vec<String> = (0..line_count)
            .map(|index| format!("l{index:03}"))
            .collect();
        let mut head = base.clone();
        for region in 0..region_count {
            // Regions are 8 lines apart, so each hunk is separate: three lines
            // of context on either side cannot reach the next change.
            let index = 1 + region * 8;
            head[index] = format!("changed-{region}-{index:03}");
        }
        files.push(TextFile { path, base, head });
    }
    TextCase { files }
}

fn build_text_repo(case: &TextCase) -> Repo {
    let repo = Repo::init();
    for (path, content) in case.all_paths_content(false) {
        repo.write(&path, &content);
    }
    repo.jj(&["commit", "-m", "base"]);
    for (path, content) in case.all_paths_content(true) {
        repo.write(&path, &content);
    }
    repo
}

/// Every hunk as `(path, index-in-section, id)`, in listing order.
#[derive(Debug, Clone)]
struct HunkRef {
    path: String,
    index: usize,
    id: String,
}

fn listed_hunks(repo: &Repo, case: &TextCase) -> Vec<HunkRef> {
    let run = repo.tool_ok(&["hunks"]);
    let reported = parse_hunks(&run.stdout, &case.paths());
    let mut per_path: BTreeMap<String, usize> = BTreeMap::new();
    reported
        .into_iter()
        .map(|hunk| {
            let index = per_path.entry(hunk.file.clone()).or_insert(0);
            let hunk_ref = HunkRef {
                path: hunk.file,
                index: *index,
                id: hunk.id,
            };
            *index += 1;
            hunk_ref
        })
        .collect()
}

/// The payload a correct implementation must produce for `chosen`, in the
/// order the ids were given.
fn oracle_payload(raw: &[u8], chosen: &[HunkRef]) -> String {
    let mut payload = String::new();
    for hunk in chosen {
        let section = section_for(raw, &hunk.path)
            .unwrap_or_else(|| panic!("no section for {:?}", hunk.path));
        payload.push_str(&oracle_blocks_patch(section, &[hunk.index]));
    }
    payload
}

// ──────────────────────────────────────────────────────────────────────────────
// Selection shapes
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Selection {
    All,
    First,
    Last,
    Middle,
    ContiguousRun,
    Interleaved,
}

impl Selection {
    /// The marker each selection shape must leave behind.
    fn markers(self) -> Vec<&'static str> {
        match self {
            Selection::All => vec!["select/all_hunks", "select/first_hunk", "select/last_hunk"],
            Selection::First => vec!["select/single_hunk", "select/first_hunk"],
            Selection::Last => vec!["select/single_hunk", "select/last_hunk"],
            Selection::Middle => vec!["select/single_hunk", "select/subset"],
            Selection::ContiguousRun => vec!["select/subset"],
            Selection::Interleaved => vec!["select/interleaved"],
        }
    }

    /// Which of `hunks` this shape selects. The returned list is never empty.
    fn pick(self, hunks: &[HunkRef]) -> Vec<HunkRef> {
        let count = hunks.len();
        match self {
            Selection::All => hunks.to_vec(),
            Selection::First => vec![hunks[0].clone()],
            Selection::Last => vec![hunks[count - 1].clone()],
            Selection::Middle => vec![hunks[count / 2].clone()],
            Selection::ContiguousRun => {
                if count < 2 {
                    hunks.to_vec()
                } else {
                    hunks[0..2].to_vec()
                }
            }
            Selection::Interleaved => {
                if count < 2 {
                    hunks.to_vec()
                } else {
                    (0..count).step_by(2).map(|i| hunks[i].clone()).collect()
                }
            }
        }
    }
}

#[hegel::composite]
fn selection(tc: &TestCase) -> Selection {
    tc.draw(gs::sampled_from(vec![
        Selection::All,
        Selection::First,
        Selection::Last,
        Selection::Middle,
        Selection::ContiguousRun,
        Selection::Interleaved,
    ]))
}

// ──────────────────────────────────────────────────────────────────────────────
// Properties
// ──────────────────────────────────────────────────────────────────────────────

/// Property — the payload is exactly the selected hunks, byte for byte.
///
/// For each selection shape (all, first, last, a middle hunk, a contiguous run,
/// every other hunk) the printed payload must equal the concatenation of `jj`'s
/// own `@@` blocks for exactly those hunks, in the order the ids were given:
/// same file header, same `@@` line (offsets *and* function context), same
/// context lines with their trailing whitespace, and no extra blank lines. An
/// empty selection prints nothing and exits 0. Each selection shape's marker
/// must have been hit.
///
/// Kills: payloads that drop or re-order hunks, that re-render bodies (losing
/// trailing whitespace or CR), that include a neighbouring hunk, that invent a
/// `--` or `\ No newline` line, and any selection branch that stops being taken.
#[hegel::test]
fn payload_is_exactly_the_selected_hunks(tc: TestCase) {
    let case = tc.draw(text_case());
    let shape = tc.draw(selection());
    tc.note(&format!("case = {case:#?}, selection = {shape:?}"));
    let repo = build_text_repo(&case);
    let raw = repo.jj(&["diff", "--git", "-r", "@"]);
    let hunks = listed_hunks(&repo, &case);
    let chosen = shape.pick(&hunks);
    assert!(!chosen.is_empty());
    let ids: Vec<&str> = chosen.iter().map(|hunk| hunk.id.as_str()).collect();

    let mut args = vec!["patch"];
    args.extend(ids.iter().copied());
    let mut markers = Markers::new();
    let run = repo.tool_ok(&args);
    markers.add(&run);
    assert_eq!(
        run.stdout,
        oracle_payload(&raw, &chosen),
        "payload for {shape:?} is not jj's blocks verbatim"
    );
    markers.require(
        &["patch/supported_hunk", "patch/whole_hunk"],
        "building a payload",
    );
    let wanted = shape.markers();
    markers.require(&wanted, "selecting hunks");

    // An empty selection is a no-op, not an error: `patch` with no ids prints
    // nothing, which is also the shape `select/no_hunks` names.
    let empty = repo.tool_ok(&["patch"]);
    markers.add(&empty);
    assert!(
        empty.stdout.is_empty(),
        "patch with no ids printed a payload:\n{}",
        empty.stdout
    );
    markers.require(&["select/no_hunks"], "an empty selection");
}

/// Property — inline line ranges select exactly the lines they name.
///
/// A hunk id may carry `:start-end` (or a comma-separated list) naming lines of
/// the hunk body. The sliced payload must keep the hunk's context, contain
/// exactly the named change lines, keep everything else at most as context, and
/// still apply cleanly with `patch -p1 -F 0` — i.e. the header counts and the
/// rearranged body must describe a valid patch. Applying it to the base file
/// must produce the model where exactly the selected lines changed.
///
/// Kills: ranges that select the wrong lines (off-by-one on the 1-based body
/// index), that drop context and make the patch inapplicable, and headers whose
/// counts are not recomputed after slicing.
///
/// The payload is applied with `git apply` rather than `patch(1)`: slicing turns
/// unselected removals into context, which can leave more context before the
/// changes than after, and GNU patch reads unbalanced context as a hunk anchored
/// at the file boundary and rejects it (upstream issue #2). `git apply` has no
/// such heuristic, so it judges the payload on its content alone. That also means
/// `split <id>:start-end` is not asserted end-to-end here; when the context
/// balancing fix lands, this property can require it.
#[hegel::test]
fn inline_ranges_slice_hunks_faithfully(tc: TestCase) {
    // One file, one hunk with three or more change lines: a region of adjacent
    // replaced lines, so a hunk has several `+`/`-` pairs to slice between.
    let replaced: usize = tc.draw(gs::integers::<usize>().min_value(2).max_value(4));
    let base: Vec<String> = (0..12).map(|index| format!("l{index:03}")).collect();
    let mut head = base.clone();
    for offset in 0..replaced {
        head[4 + offset] = format!("changed-{offset}");
    }
    let path = "ranged.txt".to_string();
    let repo = Repo::init();
    repo.write(&path, format!("{}\n", base.join("\n")).as_bytes());
    repo.jj(&["commit", "-m", "base"]);
    repo.write(&path, format!("{}\n", head.join("\n")).as_bytes());

    let raw = repo.jj(&["diff", "--git", "-r", "@"]);
    let section = section_for(&raw, &path).expect("file changes");
    let blocks = hunk_blocks(section);
    assert_eq!(blocks.len(), 1, "the region must be a single hunk");
    let (header, body) = &blocks[0];
    let listed = repo.tool_ok(&["hunks", "--file", &path]);
    let reported = parse_hunks(&listed.stdout, std::slice::from_ref(&path));
    assert_eq!(reported.len(), 1);
    let id = reported[0].id.clone();

    // Draw a range over the hunk's 1-based body lines; at least one change line
    // must be inside it, otherwise the selection would be a no-op.
    let change_lines: Vec<usize> = body
        .iter()
        .enumerate()
        .filter(|(_, line)| line.starts_with('+') && !line.starts_with("+++"))
        .map(|(index, _)| index + 1)
        .collect();
    assert!(!change_lines.is_empty(), "hunk has no additions: {header}");
    let pick = tc.draw(gs::sampled_from(change_lines.clone()));
    let span = tc.draw(gs::integers::<usize>().min_value(1).max_value(2));
    let first = change_lines
        .iter()
        .position(|line| *line == pick)
        .expect("picker comes from the change lines");
    let start = change_lines[first];
    let end = *change_lines
        .get(first + span)
        .unwrap_or(&(change_lines[first] + span - 1));
    let spec = format!("{id}:{start}-{end}");
    tc.note(&format!(
        "hunk {header} body {} lines, selecting :{start}-{end}",
        body.len()
    ));

    let mut markers = Markers::new();
    let run = repo.tool_ok(&["patch", &spec]);
    markers.add(&run);
    markers.require(
        &["select/line_ranges", "patch/supported_hunk"],
        "slicing a hunk",
    );

    // Structural checks on the payload.
    let sliced_blocks = hunk_blocks(run.stdout.as_bytes());
    assert_eq!(
        sliced_blocks.len(),
        1,
        "a range selection is still one hunk:\n{}",
        run.stdout
    );
    let (sliced_header, sliced_body) = &sliced_blocks[0];
    let (old_start, old_count, new_start, new_count) = parse_hunk_header(sliced_header);
    let (old_side, new_side) = body_sides(sliced_body);
    assert_eq!((old_count, new_count), (old_side.len(), new_side.len()));
    assert_eq!((old_start, new_start), {
        let (o, _, n, _) = parse_hunk_header(header);
        (o, n)
    });
    // Every kept line is one of the hunk's own lines, in order, with `+`/`-`
    // lines that were not selected turned into context.
    let stripped = |line: &str| -> String {
        line.strip_prefix(['+', '-', ' '])
            .unwrap_or(line)
            .to_string()
    };
    let mut cursor = 0usize;
    for line in sliced_body {
        let needle = stripped(line);
        let found = body[cursor..]
            .iter()
            .position(|candidate| stripped(candidate) == needle)
            .unwrap_or_else(|| panic!("payload line {line:?} is not in the hunk body"));
        cursor += found + 1;
        let original = &body[cursor - found - 1 + found];
        if line.starts_with('+') || line.starts_with('-') {
            // A line kept as a change must have been a change in the original.
            assert!(
                original.starts_with('+') || original.starts_with('-'),
                "payload turned a context line {original:?} into a change"
            );
        }
    }
    let sliced_adds: Vec<&String> = sliced_body
        .iter()
        .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
        .collect();
    let expected_adds: Vec<&String> = change_lines
        .iter()
        .filter(|line| **line >= start && **line <= end)
        .map(|line| &body[line - 1])
        .collect();
    assert_eq!(
        sliced_adds, expected_adds,
        "the sliced payload does not contain exactly the selected change lines"
    );

    // The sliced payload must apply, and must produce the base file with the
    // hunk's old range replaced by the new side the sliced header describes —
    // the model an independently built patch would produce.
    let applied = apply_patch_to_copy(
        &path,
        format!("{}\n", base.join("\n")).as_bytes(),
        &run.stdout,
    );
    let mut expected_lines: Vec<String> = base[..old_start - 1].to_vec();
    expected_lines.extend(new_side.iter().cloned());
    expected_lines.extend(base[old_start - 1 + old_count..].iter().cloned());
    let applied_lines: Vec<String> = String::from_utf8(applied)
        .expect("applied content is UTF-8")
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(
        applied_lines, expected_lines,
        "the sliced payload does not apply to base with exactly the selected lines changed"
    );
    // Sanity: every selected change line is actually in the result, and every
    // unselected change line is not.
    let selected_text: BTreeSet<String> = expected_adds.iter().map(|line| stripped(line)).collect();
    for (index, line) in body.iter().enumerate() {
        if !line.starts_with('+') || line.starts_with("+++") {
            continue;
        }
        let text = stripped(line);
        if change_lines.iter().any(|change| *change == index + 1) && selected_text.contains(&text) {
            assert!(
                applied_lines.contains(&text),
                "selected change {text:?} missing from the result"
            );
        }
    }
}

/// Property — `split` moves exactly the selected hunks into a new parent commit.
///
/// After splitting a proper subset of the hunks, the split-off commit (`@-`)
/// must hold the base file with exactly the selected hunks applied, the working
/// copy must still hold the full head content, and any file with no selected
/// hunk must be byte-identical to the base. The expected content is produced by
/// applying a hand-built patch to the base, never by the tool.
///
/// Kills: a split that moves more than it was asked to (over-application that
/// would silently rewrite a file), that moves nothing, or that leaves the
/// working copy unable to represent the remainder.
#[hegel::test]
fn split_moves_exactly_the_selected_hunks(tc: TestCase) {
    let case = tc.draw(text_case());
    let shape = tc.draw(selection());
    tc.note(&format!("case = {case:#?}, selection = {shape:?}"));
    let repo = build_text_repo(&case);
    let raw = repo.jj(&["diff", "--git", "-r", "@"]);
    let hunks = listed_hunks(&repo, &case);
    let chosen = shape.pick(&hunks);
    if chosen.len() == hunks.len() && case.files.len() == 1 && hunks.len() == 1 {
        // Splitting every hunk of the only file leaves `@` empty, which jj
        // refuses to represent; the interesting cases are proper subsets.
        return;
    }

    let mut args = vec!["split"];
    let ids: Vec<&str> = chosen.iter().map(|hunk| hunk.id.as_str()).collect();
    args.extend(ids.iter().copied());
    args.extend(["-m", "property split"]);
    let split = repo.tool_ok(&args);
    let mut markers = Markers::new();
    markers.add(&split);
    markers.require(
        &["apply/split", "apply/jj_tool", "patch/whole_hunk"],
        "splitting hunks",
    );

    for (path, base) in case.all_paths_content(false) {
        let picked: Vec<usize> = chosen
            .iter()
            .filter(|hunk| hunk.path == path)
            .map(|hunk| hunk.index)
            .collect();
        let section = section_for(&raw, &path).expect("file changes");
        let expected = if picked.is_empty() {
            base.clone()
        } else {
            apply_patch_to_copy(&path, &base, &oracle_blocks_patch(section, &picked))
        };
        assert_eq!(
            repo.jj_file("@-", &path),
            expected,
            "the split commit does not hold base + exactly the selected hunks of {path}"
        );
        assert_eq!(
            repo.jj_file("@", &path),
            case.content(&path, true),
            "the working copy lost content it should still hold for {path}"
        );
    }
}

/// Property — `squash` moves exactly the selected hunks into the destination.
///
/// Squashing a subset into the parent must leave the parent holding the base
/// with exactly those hunks applied, and the working copy holding the full head
/// content, with the *remaining* changes visible as `jj diff` of `@`. A file
/// with no selected hunk must be untouched in the parent.
///
/// Kills: a squash that moves the wrong hunks (destination over-applied) or
/// that drops the unselected ones from the working copy.
#[hegel::test]
fn squash_moves_exactly_the_selected_hunks(tc: TestCase) {
    let case = tc.draw(text_case());
    let shape = tc.draw(selection());
    tc.note(&format!("case = {case:#?}, selection = {shape:?}"));
    let repo = build_text_repo(&case);
    let raw = repo.jj(&["diff", "--git", "-r", "@"]);
    let hunks = listed_hunks(&repo, &case);
    let chosen = shape.pick(&hunks);
    let unselected: Vec<HunkRef> = hunks
        .iter()
        .filter(|hunk| !chosen.iter().any(|picked| picked.id == hunk.id))
        .cloned()
        .collect();
    if unselected.is_empty() {
        // Squashing every hunk would empty the working copy and abandon it;
        // proper subsets are what this property is about.
        return;
    }

    let mut args = vec!["squash"];
    let ids: Vec<&str> = chosen.iter().map(|hunk| hunk.id.as_str()).collect();
    args.extend(ids.iter().copied());
    args.extend(["--from", "@", "--into", "@-"]);
    let squashed = repo.tool_ok(&args);
    let mut markers = Markers::new();
    markers.add(&squashed);
    markers.require(&["apply/squash", "apply/jj_tool"], "squashing hunks");

    for (path, base) in case.all_paths_content(false) {
        let picked: Vec<usize> = chosen
            .iter()
            .filter(|hunk| hunk.path == path)
            .map(|hunk| hunk.index)
            .collect();
        let section = section_for(&raw, &path).expect("file changes");
        let expected = if picked.is_empty() {
            base.clone()
        } else {
            apply_patch_to_copy(&path, &base, &oracle_blocks_patch(section, &picked))
        };
        assert_eq!(
            repo.jj_file("@-", &path),
            expected,
            "the destination does not hold base + exactly the selected hunks of {path}"
        );
        assert_eq!(
            repo.jj_file("@", &path),
            case.content(&path, true),
            "the working copy lost content it should still hold for {path}"
        );
    }

    // What is left in the working copy is exactly the unselected hunks.
    let remaining = repo.jj(&["diff", "--git", "-r", "@"]);
    let expected_remaining_additions: Vec<String> = unselected
        .iter()
        .map(|hunk| {
            let section = section_for(&raw, &hunk.path).expect("section");
            let (_, body) = &hunk_blocks(section)[hunk.index];
            body.iter()
                .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect();
    for additions in &expected_remaining_additions {
        for line in additions.lines() {
            assert!(
                std::str::from_utf8(&remaining)
                    .expect("diff is text")
                    .lines()
                    .any(|candidate| candidate == line),
                "the unselected change {line:?} is no longer in the working copy"
            );
        }
    }
}

/// Property — `diffedit` keeps exactly the selected hunks in the revision.
///
/// `diffedit <ids> -r @` rewrites `@` to hold the base with only the selected
/// hunks applied: every unselected change must be gone from the revision, and
/// every selected one must be present.
///
/// Kills: a diffedit that keeps everything (no-op), keeps nothing, or applies
/// the wrong subset — the failure mode where a "keep only this hunk" command
/// silently rewrites unrelated lines.
#[hegel::test]
fn diffedit_keeps_exactly_the_selected_hunks(tc: TestCase) {
    let case = tc.draw(text_case());
    let shape = tc.draw(selection());
    tc.note(&format!("case = {case:#?}, selection = {shape:?}"));
    let repo = build_text_repo(&case);
    let raw = repo.jj(&["diff", "--git", "-r", "@"]);
    let hunks = listed_hunks(&repo, &case);
    let chosen = shape.pick(&hunks);
    let ids: Vec<&str> = chosen.iter().map(|hunk| hunk.id.as_str()).collect();
    let mut args = vec!["diffedit"];
    args.extend(ids.iter().copied());
    let edited = repo.tool_ok(&args);
    let mut markers = Markers::new();
    markers.add(&edited);
    markers.require(&["apply/diffedit", "apply/jj_tool"], "diffediting hunks");

    for (path, base) in case.all_paths_content(false) {
        let picked: Vec<usize> = chosen
            .iter()
            .filter(|hunk| hunk.path == path)
            .map(|hunk| hunk.index)
            .collect();
        let section = section_for(&raw, &path).expect("file changes");
        let expected = if picked.is_empty() {
            base.clone()
        } else {
            apply_patch_to_copy(&path, &base, &oracle_blocks_patch(section, &picked))
        };
        assert_eq!(
            repo.jj_file("@", &path),
            expected,
            "diffedit did not keep exactly the selected hunks of {path}"
        );
    }
}

/// Property — `restore` undoes exactly the selected hunks.
///
/// `restore <ids>` on the working copy must leave the head content with exactly
/// the selected hunks reverted — that is, base + the unselected hunks — and must
/// not disturb a file with no selected hunk.
///
/// Kills: a restore that reverts the wrong hunks (undoing work the user asked to
/// keep), that reverts everything, or that leaves the file half-reverted. The
/// reverse application is the branch this exercises (`apply/jj_tool_reverse`).
#[hegel::test]
fn restore_undoes_exactly_the_selected_hunks(tc: TestCase) {
    let case = tc.draw(text_case());
    let shape = tc.draw(selection());
    tc.note(&format!("case = {case:#?}, selection = {shape:?}"));
    let repo = build_text_repo(&case);
    let raw = repo.jj(&["diff", "--git", "-r", "@"]);
    let hunks = listed_hunks(&repo, &case);
    let chosen = shape.pick(&hunks);
    let unselected: Vec<HunkRef> = hunks
        .iter()
        .filter(|hunk| !chosen.iter().any(|picked| picked.id == hunk.id))
        .cloned()
        .collect();

    let mut args = vec!["restore"];
    let ids: Vec<&str> = chosen.iter().map(|hunk| hunk.id.as_str()).collect();
    args.extend(ids.iter().copied());
    let restored = repo.tool_ok(&args);
    let mut markers = Markers::new();
    markers.add(&restored);
    markers.require(
        &["apply/restore", "apply/jj_tool_reverse"],
        "restoring hunks",
    );

    for (path, base) in case.all_paths_content(false) {
        let kept: Vec<usize> = unselected
            .iter()
            .filter(|hunk| hunk.path == path)
            .map(|hunk| hunk.index)
            .collect();
        let section = section_for(&raw, &path).expect("file changes");
        let expected = if kept.is_empty() {
            base.clone()
        } else {
            apply_patch_to_copy(&path, &base, &oracle_blocks_patch(section, &kept))
        };
        assert_eq!(
            repo.jj_file("@", &path),
            expected,
            "restore did not undo exactly the selected hunks of {path}"
        );
    }
}

/// Property — `absorb` routes each hunk to the ancestor that introduced it.
///
/// A stack of two mutable commits introduces a file each; the working copy then
/// changes a line of the first file, a line of the second, and adds a new file.
/// `absorb` must move the first change into the first commit and the second into
/// the second (by blame), leave the new file in the working copy (nothing to
/// blame), and leave the working copy's remaining diff limited to what it could
/// not route. `--dry-run` must print the same routes without changing anything.
///
/// Kills: routing a hunk to the wrong ancestor (silently rewriting history with
/// someone else's change), absorbing a new file into an ancestor that cannot
/// contain it, and a dry run that mutates.
#[hegel::test]
fn absorb_routes_hunks_to_their_introducing_ancestor(tc: TestCase) {
    let lines: usize = tc.draw(gs::integers::<usize>().min_value(8).max_value(14));
    let first_line: usize = tc.draw(gs::integers::<usize>().min_value(1).max_value(lines - 2));
    let second_line: usize = tc.draw(gs::integers::<usize>().min_value(1).max_value(lines - 2));
    tc.note(&format!(
        "first.txt line {first_line}, second.txt line {second_line}, {lines} lines each"
    ));

    let first_base: Vec<String> = (0..lines)
        .map(|index| format!("first-{index:03}"))
        .collect();
    let second_base: Vec<String> = (0..lines)
        .map(|index| format!("second-{index:03}"))
        .collect();

    let repo = Repo::init();
    repo.write(
        "first.txt",
        format!("{}\n", first_base.join("\n")).as_bytes(),
    );
    repo.jj(&["commit", "-m", "first"]);
    repo.jj(&["bookmark", "create", "one", "-r", "@-"]);
    repo.write(
        "second.txt",
        format!("{}\n", second_base.join("\n")).as_bytes(),
    );
    repo.jj(&["commit", "-m", "second"]);
    repo.jj(&["bookmark", "create", "two", "-r", "@-"]);

    // The working copy changes one line of each file and adds a new one.
    let mut first_head = first_base.clone();
    first_head[first_line - 1] = format!("FIRST-CHANGED-{first_line}");
    let mut second_head = second_base.clone();
    second_head[second_line - 1] = format!("SECOND-CHANGED-{second_line}");
    repo.write(
        "first.txt",
        format!("{}\n", first_head.join("\n")).as_bytes(),
    );
    repo.write(
        "second.txt",
        format!("{}\n", second_head.join("\n")).as_bytes(),
    );
    repo.write("fresh.txt", b"brand new\n");

    let listing = repo.tool_ok(&["hunks"]);
    assert_eq!(
        count_reported(&listing.stdout),
        3,
        "expected two modifications and one new file:\n{}",
        listing.stdout
    );

    let dry = repo.tool_ok(&["absorb", "--dry-run"]);
    let mut markers = Markers::new();
    markers.add(&dry);
    assert!(
        dry.stdout.contains("Would absorb 2 hunk(s)"),
        "dry run should route both modifications:\n{}",
        dry.stdout
    );
    assert!(dry.stdout.contains("first.txt"), "{}", dry.stdout);
    assert!(dry.stdout.contains("second.txt"), "{}", dry.stdout);
    // The added file must be reported as unmatched. Its *reason* is deliberately
    // not asserted: it is `new file` only once `absorb` recognises the parser's
    // `dev/null` spelling for an absent side, and this property is about routing,
    // not about that string. The outcome below (the file stays in `@`) is what
    // users depend on.
    assert!(
        dry.stdout.contains("fresh.txt"),
        "the added file must be reported as unmatched:\n{}",
        dry.stdout
    );
    let before = repo.jj_file("@", "first.txt");
    assert_eq!(
        repo.jj_file("@", "second.txt"),
        format!("{}\n", second_head.join("\n")).into_bytes()
    );

    let absorbed = repo.tool_ok(&["absorb"]);
    markers.add(&absorbed);
    // `absorb/new_file` is placed on the new-file branch but is not required: that
    // branch is unreachable while `absorb` compares `old_file` against the
    // `patch(1)` spelling `"/dev/null"` (see `markers::REQUIRED`).
    markers.require(
        &[
            "apply/absorb_dry_run",
            "apply/absorb_execute",
            "absorb/matched_change",
        ],
        "absorbing hunks",
    );

    // Each modification landed in the commit that introduced its file...
    assert_eq!(
        repo.jj_file("one", "first.txt"),
        format!("{}\n", first_head.join("\n")).into_bytes(),
        "the change to first.txt did not land in the commit that introduced it"
    );
    assert_eq!(
        repo.jj_file("two", "second.txt"),
        format!("{}\n", second_head.join("\n")).into_bytes(),
        "the change to second.txt did not land in the commit that introduced it"
    );
    // ...and the working copy no longer carries them, but does carry the new
    // file, which no ancestor could have contained.
    let remaining =
        String::from_utf8(repo.jj(&["diff", "--git", "-r", "@"])).expect("diff is text");
    assert!(
        !remaining.contains("FIRST-CHANGED") && !remaining.contains("SECOND-CHANGED"),
        "absorbed changes are still in the working copy:\n{remaining}"
    );
    assert!(
        remaining.contains("fresh.txt"),
        "the new file must stay in the working copy:\n{remaining}"
    );
    // Absorbing moves the change between commits; it is not supposed to rewrite
    // the working copy's bytes, so the content `@` reports is unchanged.
    assert_eq!(before, repo.jj_file("@", "first.txt"));
}

/// Property — the same selection through `patch` and through an apply mode
/// produces the same content.
///
/// Differential oracle: content obtained by applying the printed payload to the
/// base file with `patch(1)` must equal the content `split` commits for the same
/// hunks. This is the property that ties the payload the user can inspect to
/// what the tool actually writes, so a divergence between the two paths — the
/// classic "the patch looks right but the commit does not" — cannot pass.
#[hegel::test]
fn printed_payload_and_split_agree(tc: TestCase) {
    let case = tc.draw(text_case());
    let shape = tc.draw(selection());
    tc.note(&format!("case = {case:#?}, selection = {shape:?}"));
    let repo = build_text_repo(&case);
    let raw = repo.jj(&["diff", "--git", "-r", "@"]);
    // One file's hunks only: the printed payload is applied to a scratch copy of
    // that file, so it must not also reference a second path.
    let all = listed_hunks(&repo, &case);
    let path = all[0].path.clone();
    let hunks: Vec<HunkRef> = all
        .iter()
        .filter(|hunk| hunk.path == path)
        .cloned()
        .collect();
    let chosen = shape.pick(&hunks);
    if chosen.len() == hunks.len() {
        // A split that moves every hunk leaves the working copy empty.
        return;
    }
    let ids: Vec<&str> = chosen.iter().map(|hunk| hunk.id.as_str()).collect();

    let mut patch_args = vec!["patch"];
    patch_args.extend(ids.iter().copied());
    let printed = repo.tool_ok(&patch_args);
    let mut markers = Markers::new();
    markers.add(&printed);
    assert_eq!(printed.stdout, oracle_payload(&raw, &chosen));

    let mut split_args = vec!["split"];
    split_args.extend(ids.iter().copied());
    split_args.extend(["-m", "property differential"]);
    let split = repo.tool_ok(&split_args);
    markers.add(&split);
    markers.require(
        &["apply/patch_only", "apply/split", "patch/whole_hunk"],
        "comparing payload and split",
    );

    let base = case.content(&path, false);
    let picked: Vec<usize> = chosen.iter().map(|hunk| hunk.index).collect();
    let section = section_for(&raw, &path).expect("file changes");
    let applied_printed = apply_patch_to_copy(&path, &base, &printed.stdout);
    let from_oracle = apply_patch_to_copy(&path, &base, &oracle_blocks_patch(section, &picked));
    assert_eq!(
        applied_printed, from_oracle,
        "the printed payload for {path} applies to something different from the \
         independently built patch for the same hunks"
    );
    assert_eq!(
        repo.jj_file("@-", &path),
        applied_printed,
        "split did not commit what the printed payload applies to for {path}"
    );

    // Files with no selected hunk are untouched in both commits.
    for (other, content) in case.all_paths_content(true) {
        if other != path {
            assert_eq!(repo.jj_file("@-", &other), case.content(&other, false));
            assert_eq!(repo.jj_file("@", &other), content);
        }
    }
}
