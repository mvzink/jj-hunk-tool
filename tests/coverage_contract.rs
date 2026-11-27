//! The coverage-marker contract.
//!
//! `src/markers.rs` declares a closed set of named markers and a `REQUIRED`
//! subset — the behaviours the test suite promises to exercise. This test drives
//! every one of them through deterministic scenarios and then asserts the
//! contract: `REQUIRED ⊆ hit`. When coverage silently drops — a generator stops
//! producing rename-shaped diffs, an apply mode stops calling a helper, a branch
//! becomes unreachable — this test fails and names the missing marker, instead
//! of the suite staying green while testing less.
//!
//! Two kinds of scenario, because two kinds of marker:
//!
//! * Most markers are reached by running the real binary in a real `jj`
//!   repository, and are collected from its stderr (`JJ_HUNK_TOOL_MARKERS`).
//! * `parse/unsupported_copy` and `parse/unsupported_similarity` describe diff
//!   shapes `jj` itself never emits (it does not do copy detection, and it
//!   prints no `similarity index` line). No `jj` repository can reach them, so
//!   they are reached in-process by feeding synthetic diff text through the same
//!   library entry point the binary uses — `diff::parse_diff` +
//!   `markers::observe_hunks`.
//!
//! The deliberate-failure demonstration for this contract (removing one `hit!`
//! call) is recorded in the run report; the assertion below is what fails.
//!
//! Run only this contract with:
//!
//! ```sh
//! cargo nextest run --test coverage_contract
//! ```

mod harness;

use harness::*;
use jj_hunk_tool::markers::{self, Marker};

// ──────────────────────────────────────────────────────────────────────────────
// Scenario helpers
// ──────────────────────────────────────────────────────────────────────────────

/// A file with `count` distinct lines.
fn lines(prefix: &str, count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("{prefix}{index:02}"))
        .collect()
}

fn write_lines(repo: &Repo, path: &str, lines: &[String]) {
    repo.write(path, format!("{}\n", lines.join("\n")).as_bytes());
}

/// Ids of every listed hunk, in listing order, restricted to `path`.
fn ids_for(repo: &Repo, path: &str) -> Vec<String> {
    let listed = repo.tool_ok(&["hunks", "--file", path]);
    parse_hunks(&listed.stdout, std::slice::from_ref(&path.to_string()))
        .into_iter()
        .map(|hunk| hunk.id)
        .collect()
}

/// A repository with one 12-line file and a given set of single-line changes in
/// the working copy: the smallest useful apply-mode fixture.
fn changed_repo(changes: &[(usize, &str)]) -> Repo {
    let repo = Repo::init();
    let base = lines("c", 12);
    write_lines(&repo, "f.txt", &base);
    repo.jj(&["commit", "-m", "base"]);
    let mut head = base;
    for (index, text) in changes {
        head[*index] = (*text).to_string();
    }
    write_lines(&repo, "f.txt", &head);
    repo
}

// ──────────────────────────────────────────────────────────────────────────────
// Scenarios
// ──────────────────────────────────────────────────────────────────────────────

/// Every file and hunk shape, plus the refusal paths for shapes hunk-level
/// operations cannot represent.
fn shapes(markers: &mut Markers) {
    let repo = Repo::init();

    let modified = lines("mod", 10);
    let deleted = lines("del", 10);
    let renamed = lines("ren", 10);
    let moded = lines("mode", 8);
    let insert_target = lines("ins", 12);
    let delete_target = lines("dlo", 12);
    // Two identical 7-line windows, so the two hunks have identical bodies and
    // therefore identical id prefixes: the second must be suffixed `-2`.
    let duplicate = [
        "ctx-a", "ctx-b", "ctx-c", "old", "ctx-d", "ctx-e", "ctx-f", "filler-1", "filler-2",
        "filler-3", "ctx-a", "ctx-b", "ctx-c", "old", "ctx-d", "ctx-e", "ctx-f",
    ];
    let mut duplicate_head = duplicate;

    write_lines(&repo, "mod.txt", &modified);
    write_lines(&repo, "del.txt", &deleted);
    write_lines(&repo, "ren.txt", &renamed);
    write_lines(&repo, "mode.txt", &moded);
    write_lines(&repo, "ins.txt", &insert_target);
    write_lines(&repo, "dlo.txt", &delete_target);
    write_lines(
        &repo,
        "dup.txt",
        &duplicate.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
    );
    write_lines(&repo, "nonl.txt", &["a".into(), "b".into()]);
    // Non-UTF-8, NUL-free: jj renders a text diff whose section cannot be
    // decoded, which is the decode path the tool has to skip.
    repo.write("opaque.age", b"age-encryption.org/v1\n\xff\xfeopaque\n");
    repo.jj(&["commit", "-m", "base"]);

    let mut modified_head = modified;
    modified_head[4] = "MODIFIED".into();
    write_lines(&repo, "mod.txt", &modified_head);

    repo.remove("del.txt");

    let mut renamed_head = renamed;
    renamed_head[4] = "RENAMED".into();
    repo.remove("ren.txt");
    write_lines(&repo, "renamed.txt", &renamed_head);

    let mut moded_head = moded;
    moded_head[4] = "MODED".into();
    write_lines(&repo, "mode.txt", &moded_head);
    let mode_path = repo.abs("mode.txt");
    let mut permissions = std::fs::metadata(&mode_path).expect("stat").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&mode_path, permissions).expect("chmod +x");

    // Additions-only hunk in an existing file.
    let mut insert_head = insert_target;
    insert_head.insert(6, "INSERTED".to_string());
    write_lines(&repo, "ins.txt", &insert_head);

    // Deletions-only hunk in an existing file.
    let mut delete_head = delete_target;
    delete_head.remove(6);
    delete_head.remove(6);
    write_lines(&repo, "dlo.txt", &delete_head);

    duplicate_head[3] = "new";
    duplicate_head[13] = "new";
    write_lines(
        &repo,
        "dup.txt",
        &duplicate_head
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
    );

    // No trailing newline on either side, changed on the last line so the hunk
    // really carries the `\ No newline at end of file` markers.
    repo.write("nonl.txt", b"a\nB");

    // The undecodable file has to *change* to appear in the diff at all: it is
    // rendered as a text diff by jj and its section cannot be decoded, which is
    // the path that must skip it rather than abort or decode lossily.
    repo.write(
        "opaque.age",
        b"age-encryption.org/v1\n\xff\xfeopaque-changed\n",
    );

    repo.write("added.txt", b"added-1\nadded-2\n");

    let listing = repo.tool_ok(&["hunks"]);
    markers.add(&listing);
    assert!(
        listing.stdout.contains("dup.txt") && listing.stdout.contains("-2"),
        "the duplicate-body case must produce a suffixed id:\n{}",
        listing.stdout
    );
    assert!(
        !listing.stdout.contains("opaque.age"),
        "the undecodable file must not be listed:\n{}",
        listing.stdout
    );

    // A supported selection, a line-range selection, and a reverse payload.
    let good = ids_for(&repo, "mod.txt");
    assert_eq!(good.len(), 1);
    markers.add(&repo.tool_ok(&["patch", &good[0]]));
    // A line range that includes the change is a real payload.
    markers.add(&repo.tool_ok(&["patch", &format!("{}:3-5", good[0])]));
    // A range that covers only context lines slices to an all-context hunk,
    // which no applier accepts: the validator refuses it instead of printing a
    // patch that cannot be applied.
    let empty_range = repo.tool(&["patch", &format!("{}:1-2", good[0])]);
    markers.add(&empty_range);
    assert!(
        !empty_range.success && empty_range.stderr.contains("no changes in the hunk"),
        "a range with no change lines must be refused, got: {}",
        empty_range.stderr
    );
    markers.add(&repo.tool_ok(&["patch", &good[0], "--reverse"]));

    // Refusals: a rename and a mode change both carry metadata a `@@` block
    // cannot reproduce.
    let rename_id = ids_for(&repo, "renamed.txt");
    assert_eq!(rename_id.len(), 1, "the renamed file has one hunk");
    let refused = repo.tool(&["patch", &rename_id[0]]);
    markers.add(&refused);
    assert!(
        !refused.success && refused.stderr.contains("not supported"),
        "a rename hunk must be refused, got: {}",
        refused.stderr
    );
    let mode_id = ids_for(&repo, "mode.txt");
    let mode_refused = repo.tool(&["patch", &mode_id[0]]);
    markers.add(&mode_refused);
    assert!(!mode_refused.success);
    assert!(
        mode_refused.stderr.contains("old mode") || mode_refused.stderr.contains("new mode"),
        "a mode-change hunk must be refused for the mode metadata, got: {}",
        mode_refused.stderr
    );
}

/// A repository with four separate hunks, for the selection branches.
fn selection(markers: &mut Markers) {
    let repo = Repo::init();
    let base = lines("s", 40);
    write_lines(&repo, "sel.txt", &base);
    repo.jj(&["commit", "-m", "base"]);
    let mut head = base;
    for offset in 0..4 {
        head[1 + offset * 9] = format!("CHANGED{offset}");
    }
    write_lines(&repo, "sel.txt", &head);
    let ids = ids_for(&repo, "sel.txt");
    assert_eq!(ids.len(), 4, "expected four separate hunks");

    // all
    let mut args = vec!["patch"];
    args.extend(ids.iter().map(String::as_str));
    markers.add(&repo.tool_ok(&args));
    // first / last / middle / subset / interleaved
    markers.add(&repo.tool_ok(&["patch", &ids[0]]));
    markers.add(&repo.tool_ok(&["patch", &ids[3]]));
    markers.add(&repo.tool_ok(&["patch", &ids[1], &ids[2]]));
    markers.add(&repo.tool_ok(&["patch", &ids[0], &ids[2]]));
    // none
    markers.add(&repo.tool_ok(&["patch"]));
}

/// Every apply mode, each on its own fixture.
fn apply_modes(markers: &mut Markers) {
    // split
    let repo = changed_repo(&[(1, "A"), (9, "B")]);
    let ids = ids_for(&repo, "f.txt");
    markers.add(&repo.tool_ok(&["split", &ids[0], "-m", "split"]));

    // split --parallel
    let repo = changed_repo(&[(1, "A"), (9, "B")]);
    let ids = ids_for(&repo, "f.txt");
    markers.add(&repo.tool_ok(&["split", &ids[0], "-m", "parallel", "--parallel"]));

    // squash
    let repo = changed_repo(&[(1, "A"), (9, "B")]);
    let ids = ids_for(&repo, "f.txt");
    markers.add(&repo.tool_ok(&["squash", &ids[0], "--from", "@", "--into", "@-"]));

    // diffedit
    let repo = changed_repo(&[(1, "A"), (9, "B")]);
    let ids = ids_for(&repo, "f.txt");
    markers.add(&repo.tool_ok(&["diffedit", &ids[0]]));

    // restore (reverse application inside `_jj-tool`)
    let repo = changed_repo(&[(1, "A"), (9, "B")]);
    let ids = ids_for(&repo, "f.txt");
    markers.add(&repo.tool_ok(&["restore", &ids[0]]));
}

/// Absorb routing: every outcome it can report.
fn absorb(markers: &mut Markers) {
    // Two mutable commits introduce a file each; the working copy changes one
    // line of each and adds a file no ancestor could contain.
    let repo = Repo::init();
    let first = lines("f", 12);
    let second = lines("s", 12);
    write_lines(&repo, "first.txt", &first);
    repo.jj(&["commit", "-m", "first"]);
    write_lines(&repo, "second.txt", &second);
    repo.jj(&["commit", "-m", "second"]);
    let mut first_head = first;
    first_head[4] = "FIRST-CHANGED".into();
    let mut second_head = second;
    second_head[4] = "SECOND-CHANGED".into();
    write_lines(&repo, "first.txt", &first_head);
    write_lines(&repo, "second.txt", &second_head);
    repo.write("fresh.txt", b"brand new\n");

    markers.add(&repo.tool_ok(&["absorb", "--dry-run"]));
    let executed = repo.tool_ok(&["absorb"]);
    markers.add(&executed);
    assert!(
        executed.stdout.contains("Absorbed"),
        "the execute run must report what it absorbed:\n{}",
        executed.stdout
    );

    // A pure insertion (no deletions to blame) falls back to the most recent
    // mutable ancestor that touched the file.
    let repo = Repo::init();
    let base = lines("t", 12);
    write_lines(&repo, "touched.txt", &base);
    repo.jj(&["commit", "-m", "introduce"]);
    write_lines(&repo, "touched.txt", &base);
    let mut appended = base;
    appended.push("APPENDED".into());
    write_lines(&repo, "touched.txt", &appended);
    let dry = repo.tool_ok(&["absorb", "--dry-run"]);
    markers.add(&dry);
    assert!(
        dry.stdout.contains("touched.txt") && dry.stdout.contains("Would absorb"),
        "a pure insertion must be routed by file fallback:\n{}",
        dry.stdout
    );

    // Two mutable ancestors introduced the two lines this single hunk deletes.
    let repo = Repo::init();
    let base = lines("m", 12);
    write_lines(&repo, "mixed.txt", &base);
    repo.jj(&["commit", "-m", "c0"]);
    let mut one = base.clone();
    one[4] = "one".into();
    write_lines(&repo, "mixed.txt", &one);
    repo.jj(&["commit", "-m", "c1"]);
    let mut two = one.clone();
    two[5] = "two".into();
    write_lines(&repo, "mixed.txt", &two);
    repo.jj(&["commit", "-m", "c2"]);
    let mut both = two;
    both[4] = "X".into();
    both[5] = "Y".into();
    write_lines(&repo, "mixed.txt", &both);
    let dry = repo.tool_ok(&["absorb", "--dry-run"]);
    markers.add(&dry);
    assert!(
        dry.stdout.contains("Ambiguous"),
        "a hunk whose deletions blame two ancestors must be ambiguous:\n{}",
        dry.stdout
    );

    // A hunk whose deleted line was introduced by an *immutable* commit, in a
    // file no mutable ancestor touched: nothing overlaps, so it stays.
    let repo = Repo::init();
    let frozen = lines("z", 12);
    write_lines(&repo, "frozen.txt", &frozen);
    write_lines(&repo, "later.txt", &lines("l", 12));
    repo.jj(&["commit", "-m", "immutable base"]);
    repo.jj(&["bookmark", "create", "main", "-r", "@-"]);
    // Absorb only considers `immutable_heads()..(source-)`; teaching this
    // repository that `main` is the immutable head is what makes the base
    // commit immutable, which is the only way this branch is reachable.
    repo.jj_config_repo(r#"revset-aliases."immutable_heads()""#, "main");
    let mut later = lines("l", 12);
    later[4] = "LATER".into();
    write_lines(&repo, "later.txt", &later);
    repo.jj(&["commit", "-m", "mutable"]);
    let mut frozen_head = frozen;
    frozen_head[4] = "FROZEN".into();
    write_lines(&repo, "frozen.txt", &frozen_head);
    let mut later_head = later;
    later_head[4] = "LATER-CHANGED".into();
    write_lines(&repo, "later.txt", &later_head);
    let dry = repo.tool_ok(&["absorb", "--dry-run"]);
    markers.add(&dry);
    assert!(
        dry.stdout.contains("no overlapping ancestor hunk"),
        "a hunk blaming an immutable commit must stay unmatched:\n{}",
        dry.stdout
    );
}

/// The `_jj-tool` protocol, driven directly: forward, reverse, a patch that
/// cannot apply, and the missing-configuration error.
fn tool_protocol(markers: &mut Markers) {
    let scratch = tempfile::tempdir().expect("scratch dir");
    let left = scratch.path().join("left");
    let right = scratch.path().join("right");
    std::fs::create_dir_all(&left).expect("left");
    std::fs::create_dir_all(&right).expect("right");
    let base: Vec<u8> = b"one\ntwo\nthree\n".to_vec();
    let head: Vec<u8> = b"one\nTWO\nthree\n".to_vec();
    std::fs::write(left.join("p.txt"), &base).expect("left file");
    std::fs::write(right.join("p.txt"), &head).expect("right file");
    let patch = scratch.path().join("payload.diff");
    std::fs::write(
        &patch,
        b"--- a/p.txt\n+++ b/p.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n",
    )
    .expect("patch file");

    // `_jj-tool` is run inside a jj repo by jj itself, but needs nothing from
    // one: it resets `right` to `left` and applies the payload.
    let repo = Repo::init();
    let forward = repo.tool_env(
        &["_jj-tool", left.to_str().unwrap(), right.to_str().unwrap()],
        &[("JJ_HUNK_TOOL_PATCH", patch.to_str().unwrap())],
    );
    markers.add(&forward);
    assert!(
        forward.success,
        "_jj-tool forward apply failed: {}",
        forward.stderr
    );
    assert_eq!(
        std::fs::read(right.join("p.txt")).expect("read right"),
        head,
        "the forward apply must leave the payload applied"
    );

    // Reverse: `_jj-tool` always resets `right` to `left` first, so a reversed
    // payload needs a `left` that already holds the state the payload would
    // produce — that is how `jj restore` drives it.
    let reverse_left = scratch.path().join("reverse-left");
    let reverse_right = scratch.path().join("reverse-right");
    std::fs::create_dir_all(&reverse_left).expect("reverse left");
    std::fs::create_dir_all(&reverse_right).expect("reverse right");
    std::fs::write(reverse_left.join("p.txt"), &head).expect("reverse left file");
    let reverse = repo.tool_env(
        &[
            "_jj-tool",
            reverse_left.to_str().unwrap(),
            reverse_right.to_str().unwrap(),
        ],
        &[
            ("JJ_HUNK_TOOL_PATCH", patch.to_str().unwrap()),
            ("JJ_HUNK_TOOL_REVERSE", "1"),
        ],
    );
    markers.add(&reverse);
    assert!(
        reverse.success,
        "_jj-tool reverse apply failed: {}",
        reverse.stderr
    );
    assert_eq!(
        std::fs::read(reverse_right.join("p.txt")).expect("read reverse right"),
        base,
        "the reverse apply must undo the payload"
    );

    // A payload that cannot apply: the protocol must fail, not silently accept.
    let garbage = scratch.path().join("garbage.diff");
    std::fs::write(
        &garbage,
        b"--- a/p.txt\n+++ b/p.txt\n@@ -9,1 +9,1 @@\n-nope\n+NOPE\n",
    )
    .expect("garbage patch");
    let failed = repo.tool_env(
        &["_jj-tool", left.to_str().unwrap(), right.to_str().unwrap()],
        &[("JJ_HUNK_TOOL_PATCH", garbage.to_str().unwrap())],
    );
    markers.add(&failed);
    assert!(
        !failed.success && failed.stderr.contains("patch failed to apply"),
        "an inapplicable payload must fail loudly: {}",
        failed.stderr
    );

    // No `git` on PATH at all: the applier is a runtime dependency, so a
    // missing binary must be reported, not panicked over or silently skipped.
    let empty_path = scratch.path().join("empty-path");
    std::fs::create_dir_all(&empty_path).expect("empty PATH dir");
    let no_git = repo.tool_env(
        &["_jj-tool", left.to_str().unwrap(), right.to_str().unwrap()],
        &[
            ("JJ_HUNK_TOOL_PATCH", patch.to_str().unwrap()),
            ("PATH", empty_path.to_str().unwrap()),
        ],
    );
    markers.add(&no_git);
    assert!(
        !no_git.success && no_git.stderr.contains("no `git` executable found"),
        "a missing git binary must name itself: {}",
        no_git.stderr
    );

    // No payload configured at all.
    let unset = repo.tool(&["_jj-tool", left.to_str().unwrap(), right.to_str().unwrap()]);
    markers.add(&unset);
    assert!(
        !unset.success && unset.stderr.contains("JJ_HUNK_TOOL_PATCH"),
        "a missing payload must name the environment variable: {}",
        unset.stderr
    );
}

/// The payload validator's rejection rules, driven in-process. Each shape here is
/// one an applier would accept and mis-read, or reject without naming the hunk,
/// and each must fail loudly and name it.
fn payload_validation(markers: &mut Markers) {
    fn section(header: &str, body: &[&str]) -> String {
        let mut patch = String::from("--- a/v.txt\n+++ b/v.txt\n");
        patch.push_str(header);
        patch.push('\n');
        for line in body {
            patch.push_str(line);
            patch.push('\n');
        }
        patch
    }

    let cases: &[(&str, String)] = &[
        // Header counts that do not describe the body.
        (
            "header claims",
            section("@@ -1,3 +1,3 @@", &[" a", "-b", "+B"]),
        ),
        // More than three context lines on one side.
        (
            "context line(s) before",
            section(
                "@@ -1,6 +1,6 @@",
                &[" a", " b", " c", " d", "-e", "+E", " f"],
            ),
        ),
        // Uneven context: the shape GNU patch reads as a boundary anchor.
        (
            "context line(s) before the changes and 1 after",
            section("@@ -1,5 +1,5 @@", &[" a", " b", " c", "-d", "+D", " e"]),
        ),
        // A hunk with no context beside a hunk of the same file that has some.
        ("no context lines at all", {
            let mut patch = section("@@ -2,3 +2,3 @@", &[" a", "-b", "+B", " c"]);
            patch.push_str("--- a/v.txt\n+++ b/v.txt\n@@ -20,1 +20,1 @@\n-x\n+X\n");
            patch
        }),
        // A block that changes nothing.
        (
            "no changes in the hunk",
            section("@@ -2,3 +2,3 @@", &[" one", " two", " three"]),
        ),
    ];
    for (expected, patch) in cases {
        let error = jj_hunk_tool::validate::validate_patch(patch)
            .expect_err("the validator must reject this shape")
            .to_string();
        assert!(
            error.contains(expected),
            "expected an error containing {expected:?}, got: {error}"
        );
    }

    // And a well-formed payload passes, in-process, hitting the accept marker.
    jj_hunk_tool::validate::validate_patch(&section("@@ -1,3 +1,3 @@", &[" a", "-b", "+B", " c"]))
        .expect("a balanced hunk with context is valid");
    markers.add_names(&markers::hit_names());
}

/// Every error path a user can reach.
fn errors(markers: &mut Markers) {
    let repo = changed_repo(&[(1, "A"), (9, "B")]);
    let ids = ids_for(&repo, "f.txt");

    let not_found = repo.tool(&["patch", "0000000"]);
    markers.add(&not_found);
    assert!(not_found.stderr.contains("hunk not found"));

    // No hunks selected: the apply modes treat an empty payload as an error.
    let empty = repo.tool(&["split"]);
    markers.add(&empty);
    assert!(empty.stderr.contains("no hunks selected"));

    for (spec, expected) in [
        (format!("{}:x-3", ids[0]), "invalid start number"),
        (format!("{}:1-y", ids[0]), "invalid end number"),
        (format!("{}:zz", ids[0]), "invalid line number"),
        (format!("{}:0-1", ids[0]), "range must be 1-based"),
    ] {
        let run = repo.tool(&["patch", &spec]);
        markers.add(&run);
        assert!(
            !run.success && run.stderr.contains(expected),
            "spec {spec:?} should fail with {expected:?}, got: {}",
            run.stderr
        );
    }

    // jj itself failing after the payload was built.
    let jj_failed = repo.tool(&["split", &ids[0], "-m", "x", "-A", "no-such-rev"]);
    markers.add(&jj_failed);
    assert!(
        !jj_failed.success && jj_failed.stderr.contains("jj command failed"),
        "a failing jj invocation must be reported: {}",
        jj_failed.stderr
    );
}

/// Shapes no `jj` repository can produce, fed in-process to the same library
/// entry point the binary uses.
fn synthetic_shapes(markers: &mut Markers) {
    // `jj` does no copy detection, so a `copy from`/`copy to` section can only
    // be exercised with synthetic diff text.
    let copy = "diff --git a/x.txt b/y.txt\n\
                copy from x.txt\n\
                copy to y.txt\n\
                --- a/x.txt\n\
                +++ b/y.txt\n\
                @@ -1,2 +1,2 @@\n\
                -old\n\
                +new\n\
                 ctx\n";
    // Likewise a `similarity index` preamble.
    let similarity = "diff --git a/x.txt b/x.txt\n\
                      similarity index 90%\n\
                      --- a/x.txt\n\
                      +++ b/x.txt\n\
                      @@ -1,1 +1,1 @@\n\
                      -old\n\
                      +new\n";
    for text in [copy, similarity] {
        let hunks = jj_hunk_tool::diff::parse_diff(text);
        assert_eq!(hunks.len(), 1, "synthetic diff should parse to one hunk");
        let identified = jj_hunk_tool::diff::assign_ids(&hunks);
        markers::observe_hunks(&identified);
    }
    markers.add_names(&markers::hit_names());
}

// ──────────────────────────────────────────────────────────────────────────────
// The contract
// ──────────────────────────────────────────────────────────────────────────────

#[test]
fn required_markers_are_all_exercised() {
    let mut markers = Markers::new();

    // In-process shapes first: they hit this process's registry, which is then
    // merged into the same union as the subprocess runs.
    synthetic_shapes(&mut markers);
    shapes(&mut markers);
    selection(&mut markers);
    apply_modes(&mut markers);
    absorb(&mut markers);
    tool_protocol(&mut markers);
    payload_validation(&mut markers);
    errors(&mut markers);

    // A repository with no changes anywhere: nothing to identify.
    let empty = Repo::init();
    empty.jj(&["commit", "-m", "empty"]);
    markers.add(&empty.tool_ok(&["hunks"]));

    let required: Vec<&str> = Marker::ALL
        .iter()
        .filter(|marker| markers::REQUIRED.contains(marker))
        .map(|marker| marker.name())
        .collect();
    assert!(
        required.len() >= 40,
        "the contract should be a substantial set, has {} markers: {required:?}",
        required.len()
    );

    // THE CONTRACT. Fails, naming every missing marker, when coverage drops.
    markers.require(
        &required,
        "the coverage contract (tests/coverage_contract.rs)",
    );
}
