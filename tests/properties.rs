//! Property-based tests for jj-hunk-tool, written with Hegel.
//!
//! # The bug class these generalise (issue #1)
//!
//! `jj diff --git` renders a non-UTF-8 file that contains no NUL byte as a
//! *text* diff, not as "Binary files differ". Decoding the whole diff with
//! `String::from_utf8` therefore aborted every command with
//! `invalid utf-8 sequence …` — even `hunks --file <another file>`, because
//! `--file` is applied only after the diff is decoded.
//!
//! The obvious rescue is worse than the disease: decoding *lossily* would make
//! such a file's hunks selectable with `U+FFFD` in place of its real bytes, and
//! `split`/`squash`/`restore`/`diffedit` would then write replacement
//! characters over an encrypted secret or other opaque content. The fix treats
//! an undecodable file section like jj's own binary files: omitted from the
//! listing, hence unselectable, and never rewritten.
//!
//! These properties replace one hard-coded `.age` file with generated whole
//! repositories: empty files, CRLF, files without a trailing newline, arbitrary
//! NUL-free bytes, paths with spaces and non-ASCII characters. The invariants
//! are asserted on every generated repository instead of on a single example,
//! and byte-level invariants compare real on-disk and committed bytes rather
//! than rendered output.
//!
//! # Running
//!
//! ```sh
//! cargo nextest run --test properties                              # 32 cases
//! HEGEL_TEST_CASES=512 cargo nextest run --test properties         # soak
//! HEGEL_SEED=7 cargo nextest run --test properties                 # replayable
//! ```
//!
//! `hegel.toml` sets the default case count for the `development` and `ci`
//! profiles; `HEGEL_TEST_CASES` overrides it. Every case builds a real jj
//! repository and shells out to `jj` several times, so the count stays small:
//! generator micro-efficiency is irrelevant next to subprocess cost.

// The harness (repository fixture, oracles, patch helpers) lives in
// `tests/harness/mod.rs`; it is shared with the other property files.

mod harness;

use std::collections::BTreeSet;

use harness::*;
use hegel::TestCase;
use hegel::generators as gs;

#[hegel::composite]
fn mixed_case(tc: &TestCase, spaced_text_path: bool, crlf_ok: bool) -> Case {
    let text_path = if spaced_text_path {
        tc.draw(awkward_path())
    } else {
        tc.draw(plain_path())
    };
    let base_text = tc.draw(text_content(crlf_ok));
    let mutation = tc.draw(mutation());
    let junk_path = tc.draw(awkward_path());
    let junk_content = tc.draw(non_utf8_content());
    let extras: Vec<(String, Option<Bytes>, Option<Bytes>)> = tc.draw(
        gs::vecs(hegel::tuples!(
            awkward_path(),
            gs::optional(arbitrary_content()),
            gs::optional(arbitrary_content())
        ))
        .max_size(2),
    );

    let mut seen = BTreeSet::new();
    let text = uniquify(text_path, &mut seen);
    let junk_path = uniquify(junk_path, &mut seen);
    let mut files = vec![
        FileSpec {
            path: text.clone(),
            base: Some(base_text.clone()),
            head: Some(mutate(&base_text, mutation)),
        },
        FileSpec {
            path: junk_path.clone(),
            base: None,
            head: Some(junk_content),
        },
    ];
    for (path, base, head) in extras {
        files.push(FileSpec {
            path: uniquify(path, &mut seen),
            base,
            head,
        });
    }
    Case {
        files,
        text,
        junk: junk_path,
    }
}

/// A text-only repository whose primary file provably has two hunks (the first
/// and last line of a 40-ish line file are modified, 30+ context lines apart).
/// All content is valid UTF-8, so every generated section is decodable, and all
/// of it is LF-only so the hunk bodies can be compared byte for byte (see
/// [`text_content`]).
#[hegel::composite]
fn text_case(tc: &TestCase) -> Case {
    let path = tc.draw(plain_path());
    let filler: Vec<String> = tc.draw(
        gs::vecs(gs::text().min_size(1).max_size(10).alphabet(STEM_ALPHABET))
            .min_size(30)
            .max_size(34),
    );
    let sep = "\n";
    let trailing = tc.draw(gs::booleans());
    let extras: Vec<(String, Option<Bytes>, Option<Bytes>)> = tc.draw(
        gs::vecs(hegel::tuples!(
            awkward_path(),
            gs::optional(arbitrary_lf_text_content()),
            gs::optional(arbitrary_lf_text_content())
        ))
        .max_size(1),
    );

    let mut base_lines = vec!["top".to_string()];
    base_lines.extend(filler);
    base_lines.push("bottom".to_string());
    let mut head_lines = base_lines.clone();
    head_lines[0] = "TOP".to_string();
    let last = head_lines.len() - 1;
    head_lines[last] = "BOTTOM".to_string();

    let mut base_text = base_lines.join(sep);
    let mut head_text = head_lines.join(sep);
    if trailing {
        base_text.push_str(sep);
        head_text.push_str(sep);
    }

    let mut seen = BTreeSet::new();
    let text = uniquify(path, &mut seen);
    let mut files = vec![FileSpec {
        path: text.clone(),
        base: Some(Bytes(base_text.into_bytes())),
        head: Some(Bytes(head_text.into_bytes())),
    }];
    for (path, base, head) in extras {
        files.push(FileSpec {
            path: uniquify(path, &mut seen),
            base,
            head,
        });
    }
    Case {
        files,
        text,
        // This case family is text-only on purpose (see the doc comment), so it
        // carries no non-UTF-8 file and has nothing to point `junk` at.
        junk: String::new(),
    }
}

/// A repository whose only change is an identical-content rename of a
/// non-UTF-8 file: `from` is deleted and `to` is added with exactly the same
/// bytes, so jj reports a body-less `rename from`/`rename to` section and the
/// diff contains *no* undecodable bytes at all.
///
/// That is legitimate jj output, not a bug, and it is exactly the shape that
/// used to swallow property 1's undecodable section. It is kept as its own case
/// so the no-abort guarantee is still tested on it, with the honest expectation
/// that there is nothing to decode — see
/// [`body_less_rename_section_is_handled`].
#[hegel::composite]
fn rename_case(tc: &TestCase) -> (String, String, Bytes) {
    let content = tc.draw(non_utf8_content());
    let from = tc.draw(awkward_path());
    let mut seen = BTreeSet::new();
    let from = uniquify(from, &mut seen);
    let to = uniquify(tc.draw(awkward_path()), &mut seen);
    (from, to, content)
}

// ──────────────────────────────────────────────────────────────────────────────
// Properties
// ──────────────────────────────────────────────────────────────────────────────

/// Property 1 — no command aborts on arbitrary diff bytes.
///
/// Generates repositories containing arbitrary files (non-UTF-8 without NUL,
/// empty files, no trailing newline, CRLF, spaces and non-ASCII in paths) and
/// requires every read-only command to exit 0 without a decode error, to list
/// exactly the hunks jj shows for decodable files, and to keep undecodable
/// files unlisted (so they can never be selected) rather than listing them
/// with lossy replacement characters.
///
/// Kills: the issue #1 abort (`hunks`, and every command's diff fetch), and the
/// lossy-decode "fix" that would surface U+FFFD as selectable content.
#[hegel::test]
fn commands_never_abort_on_arbitrary_diff_bytes(tc: TestCase) {
    let case = tc.draw(mixed_case(true, true));
    tc.note(&format!("case = {case:#?}"));
    let repo = build_repo(&case);
    let diff = repo.jj(&["diff", "--git", "-r", "@"]);

    // Coverage assertions — this is the "is the interesting input actually
    // present?" check, and it costs nothing extra: the diff is already fetched
    // above and parsed for the oracle below, so the coverage question is one
    // predicate over bytes we already hold (no second `jj` run).
    //
    // It is written against the added non-UTF-8 file's *own* section, not just
    // "some undecodable bytes exist", because that is the shape that can quietly
    // disappear: jj's rename detection folds a deleted/added pair with similar
    // content into a body-less `rename from`/`rename to` section. If that ever
    // happens to the added file here, the property must fail loudly instead of
    // testing nothing.
    let undecodable_sections: Vec<&[u8]> = diff_sections(&diff)
        .into_iter()
        .filter(|section| std::str::from_utf8(section).is_err())
        .collect();
    tc.note(&format!(
        "undecodable diff sections: {} of {}",
        undecodable_sections.len(),
        diff_sections(&diff).len()
    ));
    assert!(
        std::str::from_utf8(&diff).is_err(),
        "the generated diff decodes as UTF-8, so it does not exercise issue #1: {case:#?}"
    );
    let junk_section = section(&diff, &case.junk).unwrap_or_else(|| {
        panic!(
            "the added non-UTF-8 file {:?} has no diff section of its own — \
             rename/copy detection swallowed it, so this case cannot test issue #1: {case:#?}",
            case.junk
        )
    });
    assert!(
        std::str::from_utf8(junk_section).is_err(),
        "the added non-UTF-8 file {:?} has a decodable section, so its bytes are not \
         reaching the decode path under test: {case:#?}",
        case.junk
    );

    // Every hunk jj shows for a decodable section must be listed, and nothing
    // from an undecodable section may be: the count is compared against sections
    // parsed here, so listing an undecodable file (the lossy-decode bug) shows up
    // as an overcount.
    let expected_listed: usize = diff_sections(&diff)
        .iter()
        .filter(|section| std::str::from_utf8(section).is_ok())
        .map(|section| hunk_blocks(section).len())
        .sum();

    let run = repo.tool_ok(&["hunks"]);
    assert!(
        !has_decode_error(&run.stderr),
        "hunks surfaced a decode error: {}",
        run.stderr
    );
    assert_eq!(
        count_reported(&run.stdout),
        expected_listed,
        "hunks listed a different number of hunks than jj's decodable sections hold\n{}",
        run.stdout
    );
    assert!(
        !run.stdout.contains('\u{fffd}'),
        "hunks output contains U+FFFD, so something was decoded lossily:\n{}",
        run.stdout
    );

    // The listing part of the other commands goes through the same decode.
    let absorb = repo.tool_ok(&["absorb", "--dry-run"]);
    assert!(
        !has_decode_error(&absorb.stderr),
        "absorb --dry-run surfaced a decode error: {}",
        absorb.stderr
    );

    // diffedit/restore fetch the diff with --from/--to, a separate code path.
    let from_to = repo.tool(&["diffedit", "--from", "@-", "--to", "@", "0000000"]);
    assert!(
        !has_decode_error(&from_to.stderr),
        "diffedit --from/--to surfaced a decode error: {}",
        from_to.stderr
    );
    assert!(
        from_to.stderr.contains("hunk not found"),
        "diffedit --from/--to should fail on the unknown hunk id, got:\n{}",
        from_to.stderr
    );
}

/// Property 2 — `--file` isolation.
///
/// `--file <plain text file>` must succeed and list exactly that file's hunks
/// even when the same diff also contains files that are not valid UTF-8, or are
/// otherwise pathological, and the ids must agree with the unfiltered listing.
///
/// Kills: issue #1's "even with `--file`" symptom, and any regression where
/// filtering happens before, or is hidden behind, a whole-diff decode.
#[hegel::test]
fn file_filter_isolates_plain_text_file(tc: TestCase) {
    let case = tc.draw(mixed_case(true, true));
    tc.note(&format!("case = {case:#?}"));
    let repo = build_repo(&case);
    let paths = case.paths();
    let diff = repo.jj(&["diff", "--git", "-r", "@"]);

    // Coverage assertion, free (the diff is already fetched): the *pathological
    // company* this property is about must really be in the diff. Without it the
    // property still passes when jj's rename/copy detection folds the non-UTF-8
    // file away, which would silently stop it testing `--file` isolation.
    let junk_section = section(&diff, &case.junk).unwrap_or_else(|| {
        panic!(
            "the added non-UTF-8 file {:?} has no diff section of its own, so this case does not \
             exercise `--file` isolation: {case:#?}",
            case.junk
        )
    });
    tc.note(&format!(
        "junk section {:?} undecodable: {}",
        case.junk,
        std::str::from_utf8(junk_section).is_err()
    ));
    assert!(
        std::str::from_utf8(junk_section).is_err(),
        "the file {:?} that should make this case pathological is decodable: {case:#?}",
        case.junk
    );

    let text_section = section(&diff, &case.text).expect("text file always changes");
    let expected = hunk_blocks(text_section);
    assert!(
        !expected.is_empty(),
        "text file {:?} produced no hunks",
        case.text
    );

    let filtered = repo.tool_ok(&["hunks", "--file", &case.text]);
    let reported = parse_hunks(&filtered.stdout, &paths);
    assert_eq!(
        reported.len(),
        expected.len(),
        "hunks --file {:?} listed {} hunks, jj shows {}\n{}",
        case.text,
        reported.len(),
        expected.len(),
        filtered.stdout
    );
    for hunk in &reported {
        assert_eq!(&hunk.file, &case.text);
    }

    let full = repo.tool_ok(&["hunks"]);
    let full_ids: Vec<String> = parse_hunks(&full.stdout, &paths)
        .into_iter()
        .filter(|hunk| hunk.file == case.text)
        .map(|hunk| hunk.id)
        .collect();
    let filtered_ids: Vec<String> = reported.iter().map(|hunk| hunk.id.clone()).collect();
    assert_eq!(
        full_ids, filtered_ids,
        "filtered ids differ from the unfiltered listing"
    );
}

/// Property 3 — byte-exactness of non-selected files.
///
/// Selecting a plain-text file's hunks and applying them through the jj tool
/// protocol must not change a single byte of any other file — including files
/// that are not valid UTF-8 — neither on disk nor in jj's store.
///
/// Kills: the dangerous variant of issue #1, where a lossy decode makes a
/// binary/encrypted file selectable and `split`/`squash`/`restore` write
/// U+FFFD over its bytes. Also kills silent partial-apply of the selected file.
#[hegel::test]
fn selection_never_rewrites_unselected_file_bytes(tc: TestCase) {
    let case = tc.draw(mixed_case(false, false));
    tc.note(&format!("case = {case:#?}"));
    let repo = build_repo(&case);
    let paths = case.paths();
    let diff = repo.jj(&["diff", "--git", "-r", "@"]);
    let before: Vec<Option<Vec<u8>>> = case.files.iter().map(|f| repo.read(&f.path)).collect();

    // A file whose diff section is not valid UTF-8 must not be selectable at
    // all. This is the assertion that catches the lossy-decode variant: with it,
    // such a file is listed with U+FFFD in place of its bytes, and selecting
    // those hunks writes replacement characters over the real content.
    let mut selectable_non_utf8 = Vec::new();
    for file in &case.files {
        if let Some(section) = section(&diff, &file.path)
            && std::str::from_utf8(section).is_err()
        {
            let run = repo.tool_ok(&["hunks", "--file", &file.path]);
            if count_reported(&run.stdout) > 0 {
                selectable_non_utf8.push(file.path.clone());
            }
        }
    }
    assert!(
        selectable_non_utf8.is_empty(),
        "non-UTF-8 files became selectable: {selectable_non_utf8:?}\n{}",
        repo.tool_ok(&["hunks"]).stdout
    );

    let listed = repo.tool_ok(&["hunks", "--file", &case.text]);
    let ids = hunks_for(&paths, &listed.stdout);
    assert!(!ids.is_empty(), "no hunks listed for {:?}", case.text);
    assert!(
        !listed.stdout.contains('\u{fffd}'),
        "listed hunks contain U+FFFD:\n{}",
        listed.stdout
    );

    let mut args = vec!["split"];
    args.extend(ids.iter().map(String::as_str));
    args.extend(["-m", "property split"]);
    let split = repo.tool_ok(&args);
    assert!(!has_decode_error(&split.stderr), "split: {}", split.stderr);

    for (file, original) in case.files.iter().zip(before.iter()) {
        assert_eq!(
            &repo.read(&file.path),
            original,
            "split rewrote unselected file {:?}",
            file.path
        );
        if let Some(head) = &file.head {
            assert_eq!(
                &repo.jj_file("@", &file.path),
                &head.as_slice().to_vec(),
                "file {:?} content in @ is not the original bytes",
                file.path
            );
        }
    }

    // The selected file's changes did move: the split commit holds base plus the
    // selected hunks, @ still holds the working-copy bytes.
    let primary = case.text_spec();
    let text_base = primary.base.as_ref().expect("text file has a base");
    let text_head = primary.head.as_ref().expect("text file has a head");
    assert_eq!(
        &repo.jj_file("@", &case.text),
        &text_head.as_slice().to_vec()
    );
    assert_ne!(
        &repo.jj_file("@-", &case.text),
        &text_base.as_slice().to_vec(),
        "split moved nothing for {:?}",
        case.text
    );
}

/// Property 4 — hunk accounting.
///
/// For an arbitrary text diff, the hunks reported for a file correspond exactly
/// to the `@@` blocks `jj diff --git` shows for it: same count, same body lines,
/// no dropped, duplicated or silently merged hunk; ids are unique within a file
/// and stable across repeated runs.
///
/// Kills: diffs where a hunk is lost (so selection cannot address it) or merged
/// (so selecting one hunk moves more than the user asked for), and
/// non-deterministic ids that would break scripting.
#[hegel::test]
fn hunk_accounting_matches_jj_diff_exactly(tc: TestCase) {
    let case = tc.draw(text_case());
    tc.note(&format!("case = {case:#?}"));
    let repo = build_repo(&case);
    let paths = case.paths();
    let diff = repo.jj(&["diff", "--git", "-r", "@"]);

    let first = repo.tool_ok(&["hunks"]);
    let second = repo.tool_ok(&["hunks"]);
    assert_eq!(
        first.stdout, second.stdout,
        "hunks output is not deterministic"
    );

    let mut checked = 0usize;
    for file in &case.files {
        let Some(bytes) = section(&diff, &file.path) else {
            continue;
        };
        assert!(
            std::str::from_utf8(bytes).is_ok(),
            "text case produced a non-UTF-8 section"
        );
        let expected = hunk_blocks(bytes);
        let run = repo.tool_ok(&["hunks", "--file", &file.path]);
        let reported = parse_hunks(&run.stdout, &paths);
        assert_eq!(
            reported.len(),
            expected.len(),
            "file {:?}: reported {} hunks, jj shows {}\n{}",
            file.path,
            reported.len(),
            expected.len(),
            run.stdout
        );
        let ids: BTreeSet<&str> = reported.iter().map(|hunk| hunk.id.as_str()).collect();
        assert_eq!(
            ids.len(),
            reported.len(),
            "duplicate hunk id in {:?}",
            file.path
        );
        for (report, (header, body)) in reported.iter().zip(expected.iter()) {
            assert_eq!(&report.file, &file.path);
            assert_eq!(
                &report.lines, body,
                "hunk {} ({:?}, {}) does not match jj's block",
                report.id, file.path, header
            );
        }
        checked += expected.len();
    }
    assert!(
        checked >= 2,
        "expected the two-hunk primary file, saw {checked} hunks"
    );
}

/// Property 5 — selection round-trip.
///
/// Selecting a proper subset of a file's hunks and applying it through the
/// tool's own mechanism (`patch`, then `split` via the jj tool protocol) must
/// produce exactly the content that an independently built patch for the same
/// subset produces — and that content must differ from both the base and the
/// head, so a partial apply that silently does nothing or everything is caught.
///
/// Kills: partial-apply regressions that "exit 0" while dropping or
/// over-applying a hunk, and any mismatch between what the tool prints and what
/// it commits.
#[hegel::test]
fn selected_hunks_apply_like_a_hand_built_patch(tc: TestCase) {
    let case = tc.draw(text_case());
    tc.note(&format!("case = {case:#?}"));
    let repo = build_repo(&case);
    let paths = case.paths();
    let diff = repo.jj(&["diff", "--git", "-r", "@"]);
    let primary = case.text_spec();
    let primary_path = primary.path.clone();
    let section = section(&diff, &primary_path).expect("primary file always changes");
    let base = primary
        .base
        .as_ref()
        .expect("primary file has a base")
        .as_slice()
        .to_vec();
    let head = primary
        .head
        .as_ref()
        .expect("primary file has a head")
        .as_slice()
        .to_vec();
    let blocks = hunk_blocks(section);
    assert!(
        blocks.len() >= 2,
        "primary file {primary_path:?} should have two hunks, has {}",
        blocks.len()
    );

    let listed = repo.tool_ok(&["hunks", "--file", &primary_path]);
    let reported = parse_hunks(&listed.stdout, &paths);
    assert_eq!(
        reported.len(),
        blocks.len(),
        "hunk count mismatch:\n{}",
        listed.stdout
    );

    // Exclude exactly one hunk, so the selection is a proper subset.
    let excluded: usize = tc.draw(
        gs::integers::<usize>()
            .min_value(0)
            .max_value(blocks.len() - 1),
    );
    let selected_ids: Vec<&str> = reported
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != excluded)
        .map(|(_, hunk)| hunk.id.as_str())
        .collect();
    assert!(!selected_ids.is_empty());

    let expected = apply_patch_to_copy(&primary_path, &base, &oracle_patch(section, excluded));
    assert_ne!(&expected, &base, "hand-built patch changed nothing");
    assert_ne!(&expected, &head, "hand-built patch applied every hunk");

    let mut patch_args = vec!["patch"];
    patch_args.extend(selected_ids.iter().copied());
    let tool_patch = repo.tool_ok(&patch_args);
    assert!(
        !has_decode_error(&tool_patch.stderr),
        "patch: {}",
        tool_patch.stderr
    );
    let applied = apply_patch_to_copy(&primary_path, &base, &tool_patch.stdout);
    assert_eq!(
        &applied, &expected,
        "patch output does not match an independently built patch for the same hunks"
    );

    // Same selection, but through jj: the split-off commit must hold exactly the
    // expected content.
    let mut split_args = vec!["split"];
    split_args.extend(selected_ids.iter().copied());
    split_args.extend(["-m", "property roundtrip"]);
    let split = repo.tool_ok(&split_args);
    assert!(!has_decode_error(&split.stderr), "split: {}", split.stderr);
    assert_eq!(
        &repo.jj_file("@-", &primary_path),
        &expected,
        "split commit content differs from the independently applied subset"
    );
    assert_eq!(&repo.jj_file("@", &primary_path), &head);
}

/// Property 6 — the body-less rename shape.
///
/// jj's rename detection turns a deleted/added pair with identical content into
/// a `rename from`/`rename to` section with no hunk and no body. For a
/// non-UTF-8 file that means the diff carries no undecodable bytes at all: the
/// file's bytes are not in the diff, so nothing can be decoded, listed or
/// corrupted. The property asserts the honest expectation for that shape —
/// every command still exits 0, nothing reports a decode error, the listing
/// matches the (empty) set of hunks jj shows, and no `U+FFFD` appears — rather
/// than claiming undecodable bytes are present.
///
/// Kills: the abort class on a shape that has no body to decode (e.g. a future
/// "decode only sections with hunks" tweak that mishandles metadata-only
/// sections), and it keeps the identical-content scenario covered now that
/// [`mixed_case`] deliberately avoids it.
#[hegel::test]
fn body_less_rename_section_is_handled(tc: TestCase) {
    let (from, to, content) = tc.draw(rename_case());
    tc.note(&format!("rename {from:?} -> {to:?} with {content:?}"));
    let repo = Repo::init();
    repo.write(&from, content.as_slice());
    repo.jj(&["commit", "-m", "base"]);
    repo.remove(&from);
    repo.write(&to, content.as_slice());

    let diff = repo.jj(&["diff", "--git", "-r", "@"]);

    // Coverage assertion, free (the diff is already fetched): the case must be
    // one of the two interesting shapes — either jj folded the pair into a
    // metadata-only rename (today), or it did not and the bytes are in a hunk.
    // Anything else means the generator stopped producing the scenario.
    let rename_markers = line_offsets(&diff)
        .iter()
        .filter(|(_, line)| line.starts_with(b"rename from ") || line.starts_with(b"rename to "))
        .count();
    let undecodable = diff_sections(&diff)
        .into_iter()
        .filter(|section| std::str::from_utf8(section).is_err())
        .count();
    tc.note(&format!(
        "rename markers: {rename_markers}, undecodable sections: {undecodable}"
    ));
    assert!(
        rename_markers >= 2 || undecodable >= 1,
        "expected jj to either rename-detect this pair (>=2 rename markers) or to put the bytes \
         in a hunk (>=1 undecodable section), got {rename_markers} markers and {undecodable} \
         undecodable sections:\n{}",
        String::from_utf8_lossy(&diff)
    );

    let expected_listed: usize = diff_sections(&diff)
        .into_iter()
        .filter(|section| std::str::from_utf8(section).is_ok())
        .map(|section| hunk_blocks(section).len())
        .sum();
    let run = repo.tool_ok(&["hunks"]);
    assert!(
        !has_decode_error(&run.stderr),
        "hunks surfaced a decode error on a body-less rename: {}",
        run.stderr
    );
    assert_eq!(
        count_reported(&run.stdout),
        expected_listed,
        "hunks listed a different number of hunks than jj's decodable sections hold\n{}",
        run.stdout
    );
    assert!(
        !run.stdout.contains('\u{fffd}'),
        "hunks output contains U+FFFD:\n{}",
        run.stdout
    );

    // Same for the other read-only entry point, and for the file-scoped listing
    // of the renamed-to path: no hunks exist for it, so it must list nothing
    // rather than abort.
    let absorb = repo.tool_ok(&["absorb", "--dry-run"]);
    assert!(
        !has_decode_error(&absorb.stderr),
        "absorb --dry-run surfaced a decode error on a body-less rename: {}",
        absorb.stderr
    );
    let scoped = repo.tool_ok(&["hunks", "--file", &to]);
    assert!(
        !has_decode_error(&scoped.stderr),
        "hunks --file {to:?} surfaced a decode error: {}",
        scoped.stderr
    );
    assert_eq!(
        count_reported(&scoped.stdout),
        0,
        "a body-less rename has no hunks, so --file must list none:\n{}",
        scoped.stdout
    );
}
