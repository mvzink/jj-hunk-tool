//! Property tests for the *shapes* of diffs the tool has to survive, for hunk
//! identification and numbering, and for unusual paths.
//!
//! `tests/properties.rs` generalises the non-UTF-8 bug class from issue #1. The
//! properties here generalise the rest of the input space: the file and hunk
//! shapes `jj diff --git` can produce, the ids the tool assigns, and the line
//! spans it claims.
//!
//! Each case builds one real `jj` repository holding several shapes at once, so
//! a case costs one `jj commit` plus a handful of tool invocations. The oracle
//! is `jj`'s own `diff --git` bytes re-parsed here, never the tool's output.
//!
//! Every property also asserts *markers*: the shapes it generated must have
//! been identified as those shapes (see `src/markers.rs`). A shape that quietly
//! stops being generated, or stops reaching the code that handles it, fails the
//! property by name instead of leaving it green and empty.
//!
//! # Known shapes this suite does not judge
//!
//! CRLF files: `git-surgeon`'s parser splits with `str::lines()`, which strips
//! the `\r`, so CRLF hunks are listed and re-emitted without their carriage
//! returns. That is a pre-existing bug unrelated to any marker here, so CRLF
//! cases check hunk *counts* and markers, not body bytes.

mod harness;

use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt;

use harness::*;
use hegel::TestCase;
use hegel::generators as gs;

// ──────────────────────────────────────────────────────────────────────────────
// Shape model
// ──────────────────────────────────────────────────────────────────────────────

/// What a generated case intends `jj diff` to show for one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// Existing file with both `+` and `-` lines.
    Modified,
    /// Modified file whose base and head both end without a newline, so the
    /// hunk carries `\ No newline at end of file` markers.
    ModifiedNoTrailingNewline,
    /// Modified file with CRLF line endings.
    ModifiedCrlf,
    /// Modified file whose changed line is thousands of bytes long.
    ModifiedLongLine,
    /// New file: `--- /dev/null`, an additions-only hunk with `-0,0`.
    Added,
    /// New file with no content: `jj` emits a section with no `---`/`+++` and
    /// no hunk at all.
    AddedEmpty,
    /// Deleted file: `+++ /dev/null`, a deletions-only hunk.
    Deleted,
    /// Binary file (contains a NUL byte): `Binary files … differ`, no hunk.
    Binary,
    /// Only the executable bit changed: `old mode`/`new mode`, no hunk.
    ModeOnly,
    /// Executable bit *and* content changed: mode metadata plus a hunk, which
    /// hunk-level operations cannot represent.
    ModeAndModified,
    /// Identical-content rename: `jj` emits a body-less `rename from`/`rename
    /// to` section and no hunk.
    RenamedIdentical,
    /// Rename with content changed: rename metadata plus a hunk, which
    /// hunk-level operations cannot represent.
    RenamedAndModified,
}

impl Shape {
    const ALL: [Shape; 12] = [
        Shape::Modified,
        Shape::ModifiedNoTrailingNewline,
        Shape::ModifiedCrlf,
        Shape::ModifiedLongLine,
        Shape::Added,
        Shape::AddedEmpty,
        Shape::Deleted,
        Shape::Binary,
        Shape::ModeOnly,
        Shape::ModeAndModified,
        Shape::RenamedIdentical,
        Shape::RenamedAndModified,
    ];

    /// Does `jj diff --git` show at least one `@@` block for this shape?
    fn has_hunks(self) -> bool {
        matches!(
            self,
            Shape::Modified
                | Shape::ModifiedNoTrailingNewline
                | Shape::ModifiedCrlf
                | Shape::ModifiedLongLine
                | Shape::Added
                | Shape::Deleted
                | Shape::ModeAndModified
                | Shape::RenamedAndModified
        )
    }

    /// Can a hunk of this shape be moved by the tool? Metadata-only preambles
    /// (a rename, a mode change) are rejected, because a patch of the `@@`
    /// block alone cannot reproduce them.
    fn supported(self) -> bool {
        self.has_hunks() && !matches!(self, Shape::ModeAndModified | Shape::RenamedAndModified)
    }

    /// May the listing's body lines be compared byte for byte with `jj`'s?
    fn compares_bodies(self) -> bool {
        self != Shape::ModifiedCrlf
    }

    /// The identification marker this shape must reach.
    fn marker(self) -> &'static str {
        match self {
            Shape::Modified | Shape::ModifiedCrlf | Shape::ModifiedLongLine => {
                "parse/hunk_modified"
            }
            Shape::ModifiedNoTrailingNewline => "parse/hunk_no_newline_marker",
            Shape::Added => "parse/hunk_new_file",
            Shape::Deleted => "parse/hunk_deleted_file",
            Shape::ModeAndModified => "parse/unsupported_mode_change",
            Shape::RenamedAndModified => "parse/unsupported_rename",
            // Shapes with no hunk reach no hunk-shape marker on purpose: their
            // contract is that nothing is listed, asserted per case.
            Shape::AddedEmpty | Shape::Binary | Shape::ModeOnly | Shape::RenamedIdentical => "",
        }
    }
}

/// One generated file: where it lives before and after the base commit.
#[derive(Debug, Clone)]
struct GenFile {
    shape: Shape,
    base: Option<(String, Bytes)>,
    head: Option<(String, Bytes)>,
    /// `true` when the head copy is executable but the base copy was not.
    exec_in_head: bool,
    /// The path the tool must display for this file's hunks: the new side, or
    /// the old side for a deletion.
    display: String,
}

#[derive(Debug, Clone)]
struct ShapeCase {
    files: Vec<GenFile>,
}

impl ShapeCase {
    /// Every path this case touches, in either commit.
    fn paths(&self) -> Vec<String> {
        let mut paths = Vec::new();
        for file in &self.files {
            for spec in [&file.base, &file.head].into_iter().flatten() {
                paths.push(spec.0.clone());
            }
        }
        paths.sort();
        paths.dedup();
        paths
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Generators
// ──────────────────────────────────────────────────────────────────────────────

fn text_bytes(lines: usize, tag: &str, trailing_newline: bool) -> Bytes {
    let mut text = (0..lines)
        .map(|index| format!("{tag}-{index}"))
        .collect::<Vec<_>>()
        .join("\n");
    if trailing_newline {
        text.push('\n');
    }
    Bytes(text.into_bytes())
}

/// `text_bytes`, with one line replaced so base and head provably differ.
fn mutated(bytes: &Bytes, replacement: &str) -> Bytes {
    let text = String::from_utf8(bytes.0.clone()).expect("generated text is UTF-8");
    let mut lines: Vec<&str> = text.split('\n').collect();
    let trailing = text.ends_with('\n');
    if trailing {
        lines.pop();
    }
    if !lines.is_empty() {
        lines[0] = replacement;
    }
    let mut out = lines.join("\n");
    if trailing {
        out.push('\n');
    }
    Bytes(out.into_bytes())
}

/// Replace the *last* line, keeping the absence of a trailing newline.
///
/// Changing the last line is what makes `jj` emit `\ No newline at end of file`
/// markers: a change at the top of a longer file leaves that line outside the
/// hunk's context, and then there is nothing for the marker to annotate.
fn mutated_last(bytes: &Bytes, replacement: &str) -> Bytes {
    let text = String::from_utf8(bytes.0.clone()).expect("generated text is UTF-8");
    let mut lines: Vec<&str> = text.split('\n').collect();
    let trailing = text.ends_with('\n');
    if trailing {
        lines.pop();
    }
    if let Some(last) = lines.last_mut() {
        *last = replacement;
    }
    let mut out = lines.join("\n");
    if trailing {
        out.push('\n');
    }
    Bytes(out.into_bytes())
}

/// A case always carries *every* shape, in a drawn order, with drawn content.
///
/// Drawing only some shapes per case would make the shape coverage of the suite a
/// property of the random sample: a generator change that stopped producing
/// renames would shrink the suite silently, because each case only asserts the
/// markers of the shapes it happened to draw. Carrying the whole catalogue in
/// every case makes the marker requirement absolute (see
/// [`ALL_SHAPE_MARKERS`]) while the content, sizes and paths stay generated.
#[hegel::composite]
fn shape_case(tc: &TestCase) -> ShapeCase {
    let mut shapes: Vec<Shape> = Shape::ALL.to_vec();
    // A drawn order, so the diff's file order is not always the same.
    let rotate: usize = tc.draw(
        gs::integers::<usize>()
            .min_value(0)
            .max_value(shapes.len() - 1),
    );
    shapes.rotate_left(rotate);
    let sizes: Vec<usize> = tc.draw(
        gs::vecs(gs::integers::<usize>().min_value(4).max_value(12))
            .min_size(shapes.len())
            .max_size(shapes.len()),
    );
    let long_line: usize = tc.draw(gs::integers::<usize>().min_value(200).max_value(4000));

    let mut seen = BTreeSet::new();
    let mut files: Vec<GenFile> = Vec::new();
    for (index, (shape, size)) in shapes.into_iter().zip(sizes).enumerate() {
        let tag = format!("f{index}");
        let path = uniquify(tc.draw(plain_path()), &mut seen);
        let normal = text_bytes(size, &tag, true);
        let file = match shape {
            Shape::Modified => GenFile {
                shape,
                base: Some((path.clone(), normal.clone())),
                head: Some((path.clone(), mutated(&normal, &format!("{tag}-changed")))),
                exec_in_head: false,
                display: path.clone(),
            },
            Shape::ModifiedNoTrailingNewline => {
                let base = text_bytes(size, &tag, false);
                let head = mutated_last(&base, &format!("{tag}-changed"));
                GenFile {
                    shape,
                    base: Some((path.clone(), base)),
                    head: Some((path.clone(), head)),
                    exec_in_head: false,
                    display: path.clone(),
                }
            }
            Shape::ModifiedCrlf => {
                let base = Bytes(
                    format!("{tag}-crlf-base\r\n{}\r\n", text_body(size, &tag))
                        .replace('\n', "\r\n")
                        .into_bytes(),
                );
                let head = Bytes(
                    String::from_utf8(base.0.clone())
                        .expect("text")
                        .replacen(&format!("{tag}-crlf-base"), &format!("{tag}-crlf-head"), 1)
                        .into_bytes(),
                );
                GenFile {
                    shape,
                    base: Some((path.clone(), base)),
                    head: Some((path.clone(), head)),
                    exec_in_head: false,
                    display: path.clone(),
                }
            }
            Shape::ModifiedLongLine => {
                let base = text_bytes(size, &tag, true);
                let long = "x".repeat(long_line);
                GenFile {
                    shape,
                    base: Some((path.clone(), base)),
                    head: Some((path.clone(), mutated(&normal, &long))),
                    exec_in_head: false,
                    display: path.clone(),
                }
            }
            Shape::Added => {
                // Short and deliberately unlike any deleted file in the case:
                // `jj`'s rename detection folds a delete/add pair with similar
                // content into a body-less rename, which would silently remove
                // the additions-only hunk this shape exists for.
                let head = text_bytes(1 + size % 2, &format!("added-{tag}"), true);
                GenFile {
                    shape,
                    base: None,
                    head: Some((path.clone(), head)),
                    exec_in_head: false,
                    display: path.clone(),
                }
            }
            Shape::AddedEmpty => GenFile {
                shape,
                base: None,
                head: Some((path.clone(), Bytes(Vec::new()))),
                exec_in_head: false,
                display: path.clone(),
            },
            Shape::Deleted => GenFile {
                shape,
                base: Some((
                    path.clone(),
                    text_bytes(8 + size, &format!("deleted-{tag}"), true),
                )),
                head: None,
                exec_in_head: false,
                display: path.clone(),
            },
            Shape::Binary => {
                let mut base = b"\x00binary-base-".to_vec();
                base.extend_from_slice(tag.as_bytes());
                let mut head = b"\x00binary-head-".to_vec();
                head.extend_from_slice(tag.as_bytes());
                GenFile {
                    shape,
                    base: Some((path.clone(), Bytes(base))),
                    head: Some((path.clone(), Bytes(head))),
                    exec_in_head: false,
                    display: path.clone(),
                }
            }
            Shape::ModeOnly => GenFile {
                shape,
                base: Some((path.clone(), normal.clone())),
                head: Some((path.clone(), normal.clone())),
                exec_in_head: true,
                display: path.clone(),
            },
            Shape::ModeAndModified => GenFile {
                shape,
                base: Some((path.clone(), normal.clone())),
                head: Some((path.clone(), mutated(&normal, &format!("{tag}-changed")))),
                exec_in_head: true,
                display: path.clone(),
            },
            Shape::RenamedIdentical | Shape::RenamedAndModified => {
                let to = uniquify(tc.draw(plain_path()), &mut seen);
                let head = if shape == Shape::RenamedIdentical {
                    normal.clone()
                } else {
                    mutated(&normal, &format!("{tag}-changed"))
                };
                GenFile {
                    shape,
                    base: Some((path.clone(), normal.clone())),
                    head: Some((to.clone(), head)),
                    exec_in_head: false,
                    display: to,
                }
            }
        };
        files.push(file);
    }
    ShapeCase { files }
}

/// The filler body of a CRLF file, without line endings of its own.
fn text_body(lines: usize, tag: &str) -> String {
    (1..lines)
        .map(|index| format!("{tag}-crlf-{index}"))
        .collect::<Vec<_>>()
        .join("\r\n")
}

fn build_shape_repo(case: &ShapeCase) -> Repo {
    let repo = Repo::init();
    for file in &case.files {
        if let Some((path, content)) = &file.base {
            repo.write(path, content.as_slice());
        }
    }
    repo.jj(&["commit", "-m", "base"]);
    for file in &case.files {
        if let Some((base_path, _)) = &file.base
            && file
                .head
                .as_ref()
                .is_none_or(|(head_path, _)| head_path != base_path)
        {
            repo.remove(base_path);
        }
        if let Some((path, content)) = &file.head {
            repo.write(path, content.as_slice());
            if file.exec_in_head {
                let full = repo.abs(path);
                let mut permissions = std::fs::metadata(&full)
                    .expect("stat written file")
                    .permissions();
                permissions.set_mode(0o755);
                std::fs::set_permissions(&full, permissions).expect("chmod +x");
            }
        }
    }
    repo
}

/// Every identification marker the generated shapes must reach, whichever case
/// was drawn. This is the requirement that fails when the catalogue loses a shape
/// or the code stops identifying one.
const ALL_SHAPE_MARKERS: &[&str] = &[
    "parse/hunk_new_file",
    "parse/hunk_deleted_file",
    "parse/hunk_path_change",
    "parse/hunk_additions_only",
    "parse/hunk_deletions_only",
    "parse/hunk_modified",
    "parse/hunk_no_newline_marker",
    "parse/unsupported_rename",
    "parse/unsupported_mode_change",
    "diff/section_decoded",
    "diff/sections_multiple",
    "patch/supported_hunk",
    "patch/whole_hunk",
    "patch/reverse",
    "error/unsupported_metadata",
];

// ──────────────────────────────────────────────────────────────────────────────
// Properties
// ──────────────────────────────────────────────────────────────────────────────

/// Property — a listing exists for exactly the shapes that have hunks, and it
/// says what `jj` says.
///
/// For every generated shape the property checks that `hunks` lists exactly the
/// `@@` blocks `jj diff --git` shows for that file (count, body lines, and the
/// `(+A -D)` counts in the header), that shapes with no hunks are unlisted, and
/// that ids are unique and the listing deterministic. Selecting a hunk then
/// yields either `jj`'s own block byte for byte, or a refusal naming the
/// unsupported metadata — never a truncated patch.
///
/// Kills: a hunk lost or merged so the user cannot address it (renames, mode
/// changes, additions-only `-0,0` hunks, deletions-only hunks, no-trailing-
/// newline markers, very long lines), a metadata-only file becoming selectable,
/// and any partial output before a refusal. The per-shape marker assertions kill
/// the failure mode where a shape stops being generated and the property keeps
/// passing while testing nothing.
#[hegel::test]
fn listing_matches_jj_for_every_file_shape(tc: TestCase) {
    let case = tc.draw(shape_case());
    tc.note(&format!("case = {case:#?}"));
    let repo = build_shape_repo(&case);
    let paths = case.paths();
    let raw = repo.jj(&["diff", "--git", "-r", "@"]);

    assert!(
        std::str::from_utf8(&raw).is_ok(),
        "a shape case must stay decodable end to end: {case:#?}"
    );

    let mut markers = Markers::new();
    let first = repo.tool_ok(&["hunks"]);
    markers.add(&first);
    assert!(
        !first.stderr.contains("error"),
        "hunks reported an error on a decodable diff: {}",
        first.stderr
    );
    assert!(!first.stdout.contains('\u{fffd}'));
    let second = repo.tool_ok(&["hunks"]);
    assert_eq!(
        first.stdout, second.stdout,
        "hunks output is not deterministic for {case:#?}"
    );

    let expected_total: usize = diff_sections(&raw)
        .iter()
        .map(|section| hunk_blocks(section).len())
        .sum();
    assert_eq!(
        count_reported(&first.stdout),
        expected_total,
        "the listing does not have one entry per `@@` block in jj's diff\n{}\n--- diff ---\n{}",
        first.stdout,
        String::from_utf8_lossy(&raw)
    );
    let reported = parse_hunks(&first.stdout, &paths);
    let ids: BTreeSet<&str> = reported.iter().map(|hunk| hunk.id.as_str()).collect();
    assert_eq!(ids.len(), reported.len(), "duplicate hunk id");

    for file in &case.files {
        // `--file=<path>` rather than `--file <path>`: a generated path can begin
        // with `-` (a legal file name, and one this suite deliberately generates),
        // and only the `=` form is unambiguous for clap regardless of whether the
        // CLI takes hyphen-leading values as operands.
        let file_arg = format!("--file={}", file.display);
        let scoped = repo.tool_ok(&["hunks", &file_arg]);
        markers.add(&scoped);
        let listed = parse_hunks(&scoped.stdout, &paths);
        let section = section_for(&raw, &file.display);
        let blocks = section.map(hunk_blocks).unwrap_or_default();
        tc.note(&format!(
            "{:?} {:?}: {} listed, {} jj blocks",
            file.shape,
            file.display,
            listed.len(),
            blocks.len()
        ));

        if !file.shape.has_hunks() {
            assert!(
                blocks.is_empty(),
                "{:?} should have no hunk, jj shows {}: {case:#?}",
                file.shape,
                blocks.len()
            );
            assert!(
                listed.is_empty(),
                "{:?} ({:?}) has no hunk but the listing shows {}: \
                 a shape without selectable content must not become selectable\n{}",
                file.shape,
                file.display,
                listed.len(),
                scoped.stdout
            );
            continue;
        }

        assert_eq!(
            listed.len(),
            blocks.len(),
            "{:?} ({:?}): listing has {} hunks, jj shows {}\n{}\n--- diff ---\n{}",
            file.shape,
            file.display,
            listed.len(),
            blocks.len(),
            scoped.stdout,
            String::from_utf8_lossy(&raw)
        );
        assert!(!listed.is_empty());

        if file.shape.compares_bodies() {
            for (report, (header, body)) in listed.iter().zip(blocks.iter()) {
                assert_eq!(
                    &report.lines, body,
                    "{:?} ({:?}): hunk {} body differs from jj's block {}",
                    file.shape, file.display, report.id, header
                );
                let adds = body
                    .iter()
                    .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
                    .count();
                let dels = body
                    .iter()
                    .filter(|line| line.starts_with('-') && !line.starts_with("---"))
                    .count();
                assert_eq!(
                    (report.adds, report.dels),
                    (adds, dels),
                    "{:?} ({:?}): header counts (+{} -{}) do not match the body",
                    file.shape,
                    file.display,
                    report.adds,
                    report.dels
                );
            }
        }

        // Selecting the first hunk: jj's own block, or a refusal that names the
        // metadata and prints nothing.
        let id = listed[0].id.clone();
        let patched = repo.tool(&["patch", &id]);
        markers.add(&patched);
        if file.shape.supported() {
            assert!(
                patched.success,
                "{:?} ({:?}) hunk {id} should be selectable: {}",
                file.shape, file.display, patched.stderr
            );
            markers.require(
                &["patch/supported_hunk", "patch/whole_hunk"],
                "selecting a hunk",
            );
            if file.shape.compares_bodies() {
                let section = section.expect("hunk-bearing shapes always have a section");
                let expected = oracle_payload(section, &[0]);
                assert_eq!(
                    patched.stdout, expected,
                    "{:?} ({:?}): payload is not jj's block verbatim",
                    file.shape, file.display
                );
            }
            // The reverse payload must undo the hunk: applied to the head
            // content it has to reproduce the base content. Only for shapes that
            // have both sides as text.
            if let (Some((_, base)), Some((_, head))) = (&file.base, &file.head)
                && file.shape.compares_bodies()
            {
                let reversed = repo.tool_ok(&["patch", &id, "--reverse"]);
                markers.add(&reversed);
                // The payload keeps its `+`/`-` markers; it is reversed at apply
                // time, exactly as `restore` does via `patch --reverse`.
                let undone =
                    apply_patch_reversed_to_copy(&file.display, head.as_slice(), &reversed.stdout);
                assert_eq!(
                    undone,
                    base.as_slice(),
                    "{:?} ({:?}): the reverse payload does not undo the hunk",
                    file.shape,
                    file.display
                );
            }
        } else {
            assert!(
                !patched.success,
                "{:?} ({:?}) hunk {id} must be refused, not applied: {}",
                file.shape, file.display, patched.stdout
            );
            assert!(
                patched.stdout.is_empty(),
                "{:?} ({:?}): a refused selection printed a partial patch:\n{}",
                file.shape,
                file.display,
                patched.stdout
            );
            assert!(
                patched.stderr.contains("not supported"),
                "{:?} ({:?}): refusal should name the unsupported metadata, got: {}",
                file.shape,
                file.display,
                patched.stderr
            );
            markers.require(&["error/unsupported_metadata"], "refusing a metadata hunk");
        }

        if !file.shape.marker().is_empty() {
            markers.require(&[file.shape.marker()], "identifying a generated shape");
        }
    }

    // The absolute requirement: every generated shape was identified as that
    // shape, regardless of which case this was.
    markers.require(ALL_SHAPE_MARKERS, "identifying every generated shape");
}

/// Property — every hunk header describes the lines the hunk actually covers.
///
/// `@@ -old_start,old_count +new_start,new_count @@` has to agree with the body
/// (counts) *and* with the file contents (the old side is exactly
/// `base[old_start … old_start+old_count]`, the new side exactly
/// `head[new_start … new_start+new_count]`). Hunks are generated at the start of
/// the file, at its end, and in the middle, including files whose first or last
/// line is the one changed, so context-edge offsets are exercised rather than
/// assumed.
///
/// Kills: off-by-one and uneven-context offset bugs (the `fix/uneven-context`
/// class), a re-emitted header whose counts drift from its body, and the
/// `-N,0` addition convention being written as `-N,1` or `-0,0` for a deletion.
#[hegel::test]
fn hunk_headers_describe_the_lines_they_cover(tc: TestCase) {
    let lines: usize = tc.draw(gs::integers::<usize>().min_value(6).max_value(18));
    let regions: usize = tc.draw(gs::integers::<usize>().min_value(1).max_value(3));
    let whole_file = tc.draw(gs::booleans());
    tc.note(&format!(
        "base has {lines} lines, {regions} changed region(s)"
    ));

    // Distinct lines, so a hunk's span can only match one place in the file.
    let base_lines: Vec<String> = (0..lines).map(|index| format!("line-{index:02}")).collect();
    let mut head_lines = base_lines.clone();
    if whole_file {
        // A single hunk covering the file from its first line to its last.
        for (index, line) in head_lines.iter_mut().enumerate() {
            *line = format!("changed-{index:02}");
        }
    } else {
        for region in 0..regions {
            // Regions include the first and last line by construction: the
            // offset of each region is drawn modulo the line count.
            let offset: usize = tc.draw(gs::integers::<usize>().min_value(0).max_value(lines - 1));
            head_lines[offset] = format!("changed-{region}-{offset:02}");
        }
    }
    assert_ne!(base_lines, head_lines, "the case must change something");

    let path = "spans.txt".to_string();
    let base = Bytes(format!("{}\n", base_lines.join("\n")).into_bytes());
    let head = Bytes(format!("{}\n", head_lines.join("\n")).into_bytes());
    let repo = Repo::init();
    repo.write(&path, base.as_slice());
    repo.jj(&["commit", "-m", "base"]);
    repo.write(&path, head.as_slice());

    let raw = repo.jj(&["diff", "--git", "-r", "@"]);
    let section = section_for(&raw, &path).expect("file always changes");
    let blocks = hunk_blocks(section);
    assert!(!blocks.is_empty());

    let mut markers = Markers::new();
    let listed = repo.tool_ok(&["hunks", "--file", &path]);
    markers.add(&listed);
    let reported = parse_hunks(&listed.stdout, std::slice::from_ref(&path));
    assert_eq!(reported.len(), blocks.len());

    for (index, ((header, body), report)) in blocks.iter().zip(reported.iter()).enumerate() {
        let (old_start, old_count, new_start, new_count) = parse_hunk_header(header);
        let (old_side, new_side) = body_sides(body);
        tc.note(&format!(
            "hunk {index} {header}: -{old_start},{old_count} +{new_start},{new_count}, \
             body covers {} old / {} new lines",
            old_side.len(),
            new_side.len()
        ));
        assert_eq!(
            (old_count, new_count),
            (old_side.len(), new_side.len()),
            "hunk {index} header counts disagree with its body: {header}"
        );
        assert!(
            old_start >= 1 && new_start >= 1,
            "an existing file's hunks are 1-based: {header}"
        );
        assert!(old_count > 0 && new_count > 0, "existing file: {header}");
        assert_eq!(
            &base_lines[old_start - 1..old_start - 1 + old_count],
            old_side.as_slice(),
            "hunk {index} old side does not cover the base lines its header names: {header}"
        );
        assert_eq!(
            &head_lines[new_start - 1..new_start - 1 + new_count],
            new_side.as_slice(),
            "hunk {index} new side does not cover the head lines its header names: {header}"
        );
        // The listing must show the same body, line for line.
        assert_eq!(&report.lines, body);
    }

    // Every changed line appears in exactly one hunk.
    let changed_in_body: BTreeSet<&str> = blocks
        .iter()
        .flat_map(|(_, body)| body.iter())
        .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
        .map(|line| line.trim_start_matches('+'))
        .collect();
    let changed_in_head: BTreeSet<&str> = head_lines.iter().map(String::as_str).collect();
    let expected_changed: BTreeSet<&str> = changed_in_head
        .difference(&base_lines.iter().map(String::as_str).collect())
        .copied()
        .collect();
    assert_eq!(changed_in_body, expected_changed);
    markers.require(&["parse/hunk_modified"], "listing a text hunk");
}

/// Property — hunk ids are content-addressed, unique and stable.
///
/// An id must be the first seven hex digits of the SHA-1 of the hunk's display
/// path followed by its body lines, with `-2`, `-3`, … appended to later hunks
/// that share a prefix. The property recomputes every id with its own SHA-1, so
/// an id derived from anything else (a counter, the header offsets, the whole
/// file) fails. It also checks that ids are unique across a listing, identical
/// across repeated runs, unchanged when an unrelated file changes, and different
/// when one byte of a hunk changes.
///
/// Kills: ids that drift between runs (scripting breaks), collisions that are
/// not disambiguated (selecting the wrong hunk), and ids that silently stop
/// being a function of the hunk's content and path.
#[hegel::test]
fn hunk_ids_are_content_addressed_and_stable(tc: TestCase) {
    // A file with two hunks far enough apart not to merge, plus a second file.
    let filler: Vec<String> = tc.draw(
        gs::vecs(
            gs::text()
                .min_size(1)
                .max_size(8)
                .alphabet("abcdefghijklmnopqrstuvwxyz"),
        )
        .min_size(30)
        .max_size(34),
    );
    let mut base_lines = vec!["first".to_string()];
    base_lines.extend(filler);
    base_lines.push("last".to_string());
    let mut head_lines = base_lines.clone();
    head_lines[0] = "FIRST".to_string();
    let last = head_lines.len() - 1;
    head_lines[last] = "LAST".to_string();

    let path = "ids.txt".to_string();
    let other = "other.txt".to_string();
    let base = Bytes(format!("{}\n", base_lines.join("\n")).into_bytes());
    let head = Bytes(format!("{}\n", head_lines.join("\n")).into_bytes());
    let repo = Repo::init();
    repo.write(&path, base.as_slice());
    repo.write(&other, b"other-0\nother-1\n");
    repo.jj(&["commit", "-m", "base"]);
    repo.write(&path, head.as_slice());

    let raw = repo.jj(&["diff", "--git", "-r", "@"]);
    let section = section_for(&raw, &path).expect("primary file changes");
    let blocks = hunk_blocks(section);
    assert!(
        blocks.len() >= 2,
        "expected two hunks 30+ context lines apart, saw {}",
        blocks.len()
    );

    let mut markers = Markers::new();
    let listed = repo.tool_ok(&["hunks", "--file", &path]);
    markers.add(&listed);
    let reported = parse_hunks(&listed.stdout, &[path.clone(), other.clone()]);
    assert_eq!(reported.len(), blocks.len());

    let expected = expected_ids(
        &blocks
            .iter()
            .map(|(_, body)| (path.clone(), body.clone()))
            .collect::<Vec<_>>(),
    );
    let actual: Vec<String> = reported.iter().map(|hunk| hunk.id.clone()).collect();
    assert_eq!(
        actual, expected,
        "ids are not the documented content hash of path + body lines"
    );

    // Stable across runs, and unchanged by an unrelated file.
    let again = repo.tool_ok(&["hunks", "--file", &path]);
    assert_eq!(listed.stdout, again.stdout, "ids changed between runs");
    repo.write(&other, b"other-0\nOTHER-1\nother-2\n");
    let after = repo.tool_ok(&["hunks", "--file", &path]);
    markers.add(&after);
    assert_eq!(
        listed.stdout, after.stdout,
        "ids changed when an unrelated file changed, so they are not a function \
         of the selected hunk alone"
    );

    // Changing one byte of a hunk changes its id, and nothing else's.
    let mut altered = head_lines.clone();
    altered[last] = "LAST!".to_string();
    repo.write(&path, format!("{}\n", altered.join("\n")).as_bytes());
    let altered_listing = repo.tool_ok(&["hunks", "--file", &path]);
    markers.add(&altered_listing);
    let altered_reported = parse_hunks(&altered_listing.stdout, &[path.clone(), other.clone()]);
    let altered_ids: BTreeSet<&str> = altered_reported.iter().map(|h| h.id.as_str()).collect();
    let original_ids: BTreeSet<&str> = reported.iter().map(|h| h.id.as_str()).collect();
    assert_eq!(
        original_ids.intersection(&altered_ids).count(),
        original_ids.len() - 1,
        "exactly the hunk whose body changed should get a new id"
    );
    assert!(
        altered_ids.iter().any(|id| !original_ids.contains(*id)),
        "the changed hunk kept its old id"
    );

    markers.require(&["parse/hunk_modified"], "identifying hunks");
}

/// Property — unusual paths survive listing and payload construction.
///
/// Spaces, quotes, non-ASCII, leading and trailing spaces, and names that look
/// like patch headers (`diff --git …`, `--- a`, `@@ -1 +1 @@`) are all legal in
/// a repository and all appear verbatim in `jj diff --git`. The property checks
/// that the listing still reports exactly `jj`'s hunks for each such path and
/// that a selected hunk's payload is `jj`'s block byte for byte.
///
/// Kills: path handling that loses a file whose name looks like diff syntax, and
/// quoting/escaping that mangles a path into an unmatched one (so the file's
/// hunks become unaddressable).
#[hegel::test]
fn unusual_paths_are_listed_and_selected_faithfully(tc: TestCase) {
    let paths: Vec<String> = tc.draw(gs::vecs(spicy_path()).min_size(1).max_size(3));
    let mut seen = BTreeSet::new();
    let mut files = Vec::new();
    for path in paths {
        let path = uniquify(path, &mut seen);
        let base = text_bytes(6, "spicy", true);
        let head = mutated(&base, "spicy-changed");
        files.push((path, base, head));
    }

    let repo = Repo::init();
    for (path, base, _) in &files {
        repo.write(path, base.as_slice());
    }
    repo.jj(&["commit", "-m", "base"]);
    for (path, _, head) in &files {
        repo.write(path, head.as_slice());
    }

    let known: Vec<String> = files.iter().map(|(path, _, _)| path.clone()).collect();
    let raw = repo.jj(&["diff", "--git", "-r", "@"]);
    let mut markers = Markers::new();
    let listed = repo.tool_ok(&["hunks"]);
    markers.add(&listed);
    let reported = parse_hunks(&listed.stdout, &known);
    assert_eq!(
        reported.len(),
        files.len(),
        "expected one hunk per generated path\n{}\n--- diff ---\n{}",
        listed.stdout,
        String::from_utf8_lossy(&raw)
    );

    for (path, _, _) in &files {
        let section = section_for(&raw, path).unwrap_or_else(|| {
            panic!(
                "no diff section for {path:?}:\n{}",
                String::from_utf8_lossy(&raw)
            )
        });
        let blocks = hunk_blocks(section);
        let file_arg = format!("--file={path}");
        let scoped = repo.tool_ok(&["hunks", &file_arg]);
        markers.add(&scoped);
        let mine = parse_hunks(&scoped.stdout, &known);
        assert_eq!(
            mine.len(),
            blocks.len(),
            "path {path:?}: listing has {} hunks, jj shows {}",
            mine.len(),
            blocks.len()
        );
        for (report, (header, body)) in mine.iter().zip(blocks.iter()) {
            assert_eq!(&report.file, path);
            assert_eq!(
                &report.lines, body,
                "path {path:?}: body differs from {header}"
            );
        }

        let id = mine[0].id.clone();
        let patched = repo.tool(&["patch", &id]);
        markers.add(&patched);
        assert!(
            patched.success,
            "path {path:?}: hunk {id} should be selectable: {}",
            patched.stderr
        );
        assert_eq!(
            patched.stdout,
            oracle_payload(section, &[0]),
            "path {path:?}: payload is not jj's block verbatim"
        );
    }
    markers.require(&["parse/hunk_modified"], "identifying a text hunk");
}

/// Paths that are legal on disk and in `jj diff`, but awkward for anything that
/// splits on whitespace or matches diff syntax.
#[hegel::composite]
fn spicy_path(tc: &TestCase) -> String {
    let stem: String = tc.draw(
        gs::text()
            .min_size(1)
            .max_size(5)
            .alphabet("abcdefghijklmnopqrstuvwxyz"),
    );
    let leaf = tc.draw(gs::sampled_from(vec![
        format!("{stem}.txt"),
        format!("{stem} {stem}.txt"),
        format!(" {stem}.txt"),
        format!("{stem} .txt"),
        format!("{stem}\".txt"),
        format!("{stem}é.txt"),
        format!("diff --git a/{stem}.txt"),
        format!("--- {stem}.txt"),
        format!("+++ {stem}.txt"),
        format!("@@ -1 +1 @@ {stem}.txt"),
        format!("index 0000000..{stem}.txt"),
        format!("rename from {stem}.txt"),
    ]));
    if tc.draw(gs::booleans()) {
        format!("dír/{leaf}")
    } else {
        leaf
    }
}
