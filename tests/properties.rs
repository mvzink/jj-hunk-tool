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

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use hegel::TestCase;
use hegel::generators as gs;

// ──────────────────────────────────────────────────────────────────────────────
// Test harness
// ──────────────────────────────────────────────────────────────────────────────

struct ToolRun {
    success: bool,
    stdout: String,
    stderr: String,
}

struct Repo {
    dir: tempfile::TempDir,
    binary: PathBuf,
}

impl Repo {
    fn init() -> Self {
        let dir = tempfile::tempdir().expect("create temp dir");
        let output = Command::new("jj")
            .args(["git", "init", "--no-pager"])
            .current_dir(dir.path())
            .env("JJ_CONFIG", "")
            .output()
            .expect("run jj git init");
        assert!(
            output.status.success(),
            "jj git init failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Repo {
            dir,
            binary: PathBuf::from(env!("CARGO_BIN_EXE_jj-hunk-tool")),
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn abs(&self, rel: &str) -> PathBuf {
        self.path().join(rel)
    }

    fn write(&self, rel: &str, bytes: &[u8]) {
        let path = self.abs(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|e| panic!("create {}: {e}", parent.display()));
        }
        std::fs::write(&path, bytes).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    }

    fn remove(&self, rel: &str) {
        let path = self.abs(rel);
        if path.exists() {
            std::fs::remove_file(&path)
                .unwrap_or_else(|e| panic!("remove {}: {e}", path.display()));
        }
    }

    fn read(&self, rel: &str) -> Option<Vec<u8>> {
        std::fs::read(self.abs(rel)).ok()
    }

    /// Run `jj`, assert success, return raw stdout bytes (the diff may not be
    /// valid UTF-8 — that is the point).
    fn jj(&self, args: &[&str]) -> Vec<u8> {
        let output = Command::new("jj")
            .args(args)
            .arg("--no-pager")
            .current_dir(self.path())
            .env("JJ_CONFIG", "")
            .env("EDITOR", "true")
            .output()
            .expect("run jj");
        assert!(
            output.status.success(),
            "jj {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    /// Raw bytes of `rel` at `rev`, straight from jj's object store.
    fn jj_file(&self, rev: &str, rel: &str) -> Vec<u8> {
        let output = Command::new("jj")
            .args(["file", "show", "--no-pager", "-r", rev, "--", rel])
            .current_dir(self.path())
            .env("JJ_CONFIG", "")
            .env("EDITOR", "true")
            .output()
            .expect("run jj file show");
        assert!(
            output.status.success(),
            "jj file show -r {rev} -- {rel} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    fn tool(&self, args: &[&str]) -> ToolRun {
        let output = Command::new(&self.binary)
            .args(args)
            .current_dir(self.path())
            .env("JJ_CONFIG", "")
            .env("EDITOR", "true")
            .env("RUST_BACKTRACE", "0")
            .output()
            .expect("run jj-hunk-tool");
        let stdout = String::from_utf8(output.stdout).unwrap_or_else(|e| {
            panic!(
                "jj-hunk-tool {args:?} wrote non-UTF-8 stdout: {e}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
        ToolRun {
            success: output.status.success(),
            stdout,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    fn tool_ok(&self, args: &[&str]) -> ToolRun {
        let run = self.tool(args);
        assert!(
            run.success,
            "jj-hunk-tool {args:?} failed\n--- stdout ---\n{}\n--- stderr ---\n{}",
            run.stdout, run.stderr
        );
        run
    }
}

/// The pre-fix failure mode: an `invalid utf-8 sequence …` decode error.
/// Deliberately does not match the fix's `warning: skipping non-UTF-8 file …`,
/// which contains "utf-8" but is the desired behaviour.
fn has_decode_error(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("invalid utf-8 sequence")
        || lower.contains("invalid utf8 sequence")
        || lower.contains("utf8error")
}

// ──────────────────────────────────────────────────────────────────────────────
// Input model
// ──────────────────────────────────────────────────────────────────────────────

/// Byte content that prints as a readable escaped string in failing examples.
#[derive(Clone, PartialEq, Eq)]
struct Bytes(Vec<u8>);

impl fmt::Debug for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "b\"")?;
        for &b in &self.0 {
            match b {
                b'\\' => write!(f, "\\\\")?,
                b'"' => write!(f, "\\\"")?,
                b'\n' => write!(f, "\\n")?,
                b'\r' => write!(f, "\\r")?,
                b'\t' => write!(f, "\\t")?,
                0x20..=0x7e => write!(f, "{}", b as char)?,
                _ => write!(f, "\\x{b:02x}")?,
            }
        }
        write!(f, "\"")
    }
}

impl Bytes {
    fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

/// One file's state in the base commit (`base`) and in the working copy
/// (`head`). `None` means "does not exist", so a spec models additions,
/// modifications, deletions and untouched files alike.
#[derive(Debug, Clone)]
struct FileSpec {
    path: String,
    base: Option<Bytes>,
    head: Option<Bytes>,
}

/// A generated repository: a base commit plus working-copy changes.
#[derive(Debug, Clone)]
struct Case {
    files: Vec<FileSpec>,
    /// Plain-text file that provably changes and whose path is safe for
    /// `patch -p1` (no spaces), so mutation properties can select its hunks.
    text: String,
    /// The non-UTF-8 file that is *added*, so its content must appear in the
    /// diff body. Properties assert on this file's own section: see
    /// [`mixed_case`] for why it cannot be swallowed by rename detection.
    junk: String,
}

impl Case {
    fn text_spec(&self) -> &FileSpec {
        self.files
            .iter()
            .find(|file| file.path == self.text)
            .expect("case always contains its primary text file")
    }

    fn paths(&self) -> Vec<String> {
        self.files.iter().map(|file| file.path.clone()).collect()
    }
}

fn build_repo(case: &Case) -> Repo {
    let repo = Repo::init();
    for file in &case.files {
        if let Some(base) = &file.base {
            repo.write(&file.path, base.as_slice());
        }
    }
    repo.jj(&["commit", "-m", "base"]);
    for file in &case.files {
        match &file.head {
            Some(head) => repo.write(&file.path, head.as_slice()),
            None => repo.remove(&file.path),
        }
    }
    repo
}

// ──────────────────────────────────────────────────────────────────────────────
// Generators
// ──────────────────────────────────────────────────────────────────────────────

const STEM_ALPHABET: &str = "abcdefghijklmnopqrstuvwxyz0123456789 _.-";

fn eol(crlf: bool) -> &'static str {
    if crlf { "\r\n" } else { "\n" }
}

/// Plain text: ASCII lines, optionally CRLF, optionally without a trailing
/// newline. Each line is non-empty, so the file itself is never empty: an empty
/// base file cannot be proved to change, and this is used as base content.
///
/// `allow_crlf` exists because CRLF content currently trips a *different*,
/// pre-existing bug: `git-surgeon`'s parser splits the diff with `str::lines()`,
/// which strips the `\r`, so a hunk for a CRLF file is regenerated without its
/// carriage returns and `patch -p1` refuses it ("different line endings"). That
/// affects `hunks` output, `patch` and every selection command equally on the
/// pre-fix and post-fix code, so the mutation properties below use LF-only text
/// and keep CRLF for the read-only properties, where listing still works.
#[hegel::composite]
fn text_content(tc: &TestCase, allow_crlf: bool) -> Bytes {
    let lines: Vec<String> = tc.draw(
        gs::vecs(gs::text().min_size(1).max_size(10).alphabet(STEM_ALPHABET))
            .min_size(1)
            .max_size(8),
    );
    let sep = eol(allow_crlf && tc.draw(gs::booleans()));
    let trailing = tc.draw(gs::booleans());
    let mut text = lines.join(sep);
    if trailing {
        text.push_str(sep);
    }
    Bytes(text.into_bytes())
}

/// The `age`-shaped non-UTF-8 content: not valid UTF-8 and with no NUL byte, so
/// `jj diff --git` renders it as a text diff (the issue #1 trigger). The
/// `\xff\xfe` prefix makes invalidity unconditional rather than a property of the
/// random body.
///
/// This shape is reserved for the *added* file in [`mixed_case`]. jj's rename
/// detection collapses a deleted/added pair whose contents are similar — even
/// with different bodies, and completely when they are byte-identical — into a
/// body-less `rename from`/`rename to` section, which carries no undecodable
/// bytes at all. Keeping this shape to the added file means nothing that gets
/// deleted can be its rename source. Measured against jj 0.44: an added
/// `age`-shaped file next to a deleted *blob*-shaped one (see
/// [`junk_blob_content`]) is never rename- or copy-detected, while an
/// identical-pair rename is (which [`body_less_rename_section_is_handled`]
/// covers deliberately).
#[hegel::composite]
fn non_utf8_content(tc: &TestCase) -> Bytes {
    let body: Vec<u8> = tc.draw(gs::vecs(gs::integers::<u8>().min_value(1)).max_size(24));
    let mut bytes = b"age-encryption.org/v1\n\xff\xfe".to_vec();
    bytes.extend_from_slice(&body);
    bytes.push(b'\n');
    Bytes(bytes)
}

/// Non-UTF-8 content of a *different shape* from [`non_utf8_content`], for the
/// extra files in [`mixed_case`].
///
/// It starts with a bare continuation byte (`0x80..=0xBF`), which cannot begin a
/// valid UTF-8 sequence, so invalidity does not depend on what the random body
/// happens to be — the same trap as "just draw high bytes and hope". Being a
/// different shape from [`non_utf8_content`] also keeps renames from folding an
/// extra file into the `age`-shaped one.
#[hegel::composite]
fn junk_blob_content(tc: &TestCase) -> Bytes {
    let lead: u8 = tc.draw(gs::integers::<u8>().min_value(0x80).max_value(0xbf));
    let body: Vec<u8> = tc.draw(gs::vecs(gs::integers::<u8>().min_value(1)).max_size(24));
    let mut bytes = vec![lead];
    bytes.extend_from_slice(&body);
    bytes.push(b'\n');
    Bytes(bytes)
}

/// Anything a working tree may hold, for the extra files of a case.
#[hegel::composite]
fn arbitrary_content(tc: &TestCase) -> Bytes {
    if tc.draw(gs::booleans()) {
        tc.draw(text_content(true))
    } else {
        tc.draw(junk_blob_content())
    }
}

/// LF-only text content, including the empty file (see [`text_content`] for why
/// the mutation properties avoid CRLF).
#[hegel::composite]
fn arbitrary_lf_text_content(tc: &TestCase) -> Bytes {
    if tc.draw(gs::weighted_booleans(0.9)) {
        tc.draw(text_content(false))
    } else {
        Bytes(Vec::new())
    }
}

/// A path with no spaces, safe to feed to `patch -p1` (which the tool shells
/// out to). May still be non-ASCII or live in a subdirectory.
#[hegel::composite]
fn plain_path(tc: &TestCase) -> String {
    let dir = if tc.draw(gs::booleans()) { "dír/" } else { "" };
    let stem: String = tc.draw(
        gs::text()
            .min_size(1)
            .max_size(6)
            .alphabet("abcdefghijklmnopqrstuvwxyz"),
    );
    let mark: &str = tc.draw(gs::sampled_from(vec!["", "é", "ü"]));
    format!("{dir}{stem}{mark}.txt")
}

/// A path that may contain a space. Only read-only properties use these:
/// `patch(1)` cannot open a path with a space in it at all, which breaks
/// split/squash/restore/diffedit for a reason unrelated to issue #1.
#[hegel::composite]
fn awkward_path(tc: &TestCase) -> String {
    let path = tc.draw(plain_path());
    if tc.draw(gs::weighted_booleans(0.35)) {
        let word: String = tc.draw(
            gs::text()
                .min_size(1)
                .max_size(4)
                .alphabet("abcdefghijklmnopqrstuvwxyz"),
        );
        let stem = path.trim_end_matches(".txt");
        format!("{stem} {word}.txt")
    } else {
        path
    }
}

/// How the working copy differs from the base commit.
///
/// Every variant provably changes content generated by [`text_content`], whose
/// lines are non-empty and drawn from `STEM_ALPHABET` (so no line can already
/// read `REPLACED`). A mutation that could be a no-op would make a case
/// uninteresting, so none is allowed to be one.
#[derive(Debug, Clone, Copy)]
enum Mutation {
    Append,
    Prepend,
    ReplaceFirstLine,
    DropFirstLine,
    AppendCrlf,
}

fn mutate(base: &Bytes, mutation: Mutation) -> Bytes {
    let bytes = base.as_slice();
    match mutation {
        Mutation::Append => {
            let mut out = bytes.to_vec();
            out.extend_from_slice(b"appended\n");
            Bytes(out)
        }
        Mutation::Prepend => {
            let mut out = b"prepended\n".to_vec();
            out.extend_from_slice(bytes);
            Bytes(out)
        }
        Mutation::ReplaceFirstLine => {
            let rest = match bytes.iter().position(|&b| b == b'\n') {
                Some(i) => &bytes[i + 1..],
                None => &bytes[bytes.len()..],
            };
            let mut out = b"REPLACED\n".to_vec();
            out.extend_from_slice(rest);
            Bytes(out)
        }
        Mutation::DropFirstLine => {
            let rest = match bytes.iter().position(|&b| b == b'\n') {
                Some(i) => &bytes[i + 1..],
                None => &bytes[bytes.len()..],
            };
            Bytes(rest.to_vec())
        }
        Mutation::AppendCrlf => {
            let mut out = bytes.to_vec();
            out.extend_from_slice(b"tail\r\n");
            Bytes(out)
        }
    }
}

#[hegel::composite]
fn mutation(tc: &TestCase) -> Mutation {
    tc.draw(gs::sampled_from(vec![
        Mutation::Append,
        Mutation::Prepend,
        Mutation::ReplaceFirstLine,
        Mutation::DropFirstLine,
        Mutation::AppendCrlf,
    ]))
}

/// Give a path a deterministic unique name. Deterministic suffixing (rather
/// than rejection sampling) means no generated case is ever discarded, so the
/// interesting inputs are never skipped.
fn uniquify(path: String, seen: &mut BTreeSet<String>) -> String {
    if seen.insert(path.clone()) {
        return path;
    }
    for n in 2.. {
        let candidate = format!("{path}-{n}");
        if seen.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!("unbounded suffix space")
}

/// A repository with one provably-changing plain-text file plus arbitrary
/// other files, at least one of which is changed content that is not valid
/// UTF-8 (no NUL byte).
///
/// `spaced_text_path` allows the primary file's own path to contain a space and
/// `crlf_ok` allows its content to be CRLF; only the read-only properties can
/// afford either (see [`text_content`] and [`awkward_path`]).
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
// Independent oracle: line and section handling of raw `jj diff --git` bytes
// ──────────────────────────────────────────────────────────────────────────────

fn line_offsets(bytes: &[u8]) -> Vec<(usize, &[u8])> {
    let mut lines = Vec::new();
    let mut start = 0;
    while start < bytes.len() {
        let end = bytes[start..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(bytes.len(), |i| start + i);
        lines.push((start, &bytes[start..end]));
        start = end + 1;
    }
    lines
}

/// The `diff --git a/<path> b/<path>` section for `path`, raw bytes.
fn section<'a>(diff: &'a [u8], path: &str) -> Option<&'a [u8]> {
    let marker = format!("diff --git a/{path} b/{path}");
    let lines = line_offsets(diff);
    let index = lines
        .iter()
        .position(|(_, line)| *line == marker.as_bytes())?;
    let start = lines[index].0;
    let end = lines[index + 1..]
        .iter()
        .find(|(_, line)| line.starts_with(b"diff --git "))
        .map_or(diff.len(), |(offset, _)| *offset);
    Some(&diff[start..end])
}

fn is_hunk_header(line: &[u8]) -> bool {
    line.starts_with(b"@@ -")
}

/// `(header, body lines)` for each `@@` block in a section.
fn hunk_blocks(section: &[u8]) -> Vec<(String, Vec<String>)> {
    let mut blocks: Vec<(String, Vec<String>)> = Vec::new();
    for (_, line) in line_offsets(section) {
        if is_hunk_header(line) {
            blocks.push((String::from_utf8_lossy(line).into_owned(), Vec::new()));
        } else if let Some((_, body)) = blocks.last_mut() {
            body.push(String::from_utf8_lossy(line).into_owned());
        }
    }
    blocks
}

/// Split a raw `jj diff --git` into per-file sections, independently of the
/// fix's own implementation: a section starts at a line beginning with
/// `diff --git `.
///
/// Sections are handled without knowing paths so a section that jj reports as a
/// rename or a copy (which `patch`-based selection does not support) cannot make
/// the oracle silently skip it.
fn diff_sections(diff: &[u8]) -> Vec<&[u8]> {
    let starts: Vec<usize> = line_offsets(diff)
        .iter()
        .filter(|(_, line)| line.starts_with(b"diff --git "))
        .map(|(offset, _)| *offset)
        .collect();
    starts
        .iter()
        .enumerate()
        .map(|(index, start)| {
            let end = starts.get(index + 1).copied().unwrap_or(diff.len());
            &diff[*start..end]
        })
        .collect()
}

// ──────────────────────────────────────────────────────────────────────────────
// Independent oracle: parsing the tool's own `hunks` output
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    id: String,
    file: String,
    lines: Vec<String>,
}

/// Split `hunks` output into per-hunk records.
///
/// A header looks like `<7 hex digits> <path>[ <func context>] (+A -D)`; body
/// lines look like `<n>:<diff line>`. Paths are matched against `known`, the
/// paths this case generated, taking the longest match so a path that is a
/// prefix of another cannot confuse it.
fn parse_hunks(output: &str, known: &[String]) -> Vec<Reported> {
    let mut hunks: Vec<Reported> = Vec::new();
    for line in output.lines() {
        let header = line.split_once(' ').and_then(|(id, rest)| {
            let is_id = id.len() == 7 && id.chars().all(|c| c.is_ascii_hexdigit());
            if !is_id {
                return None;
            }
            known
                .iter()
                .filter(|path| rest.starts_with(path.as_str()))
                .max_by_key(|path| path.len())
                .map(|path| (id.to_string(), path.clone()))
        });
        match header {
            Some((id, file)) => hunks.push(Reported {
                id,
                file,
                lines: Vec::new(),
            }),
            None => {
                if let Some(hunk) = hunks.last_mut()
                    && let Some((_, body)) = line.split_once(':')
                {
                    hunk.lines.push(body.to_string());
                }
            }
        }
    }
    hunks
}

/// How many hunk header lines `hunks` printed, without needing to know paths:
/// a header is a 7-hex-digit id followed by a path and the ` (+A -D)` counts.
fn count_reported(output: &str) -> usize {
    output
        .lines()
        .filter(|line| {
            line.split_once(' ').is_some_and(|(id, rest)| {
                id.len() == 7 && id.chars().all(|c| c.is_ascii_hexdigit()) && rest.contains(" (+")
            })
        })
        .count()
}

fn hunks_for(paths: &[String], output: &str) -> Vec<String> {
    parse_hunks(output, paths)
        .into_iter()
        .map(|hunk| hunk.id)
        .collect()
}

// ──────────────────────────────────────────────────────────────────────────────
// patch(1) helper for the round-trip property
// ──────────────────────────────────────────────────────────────────────────────

/// Apply `patch_text` to a scratch copy of `base` and return the resulting
/// bytes of `rel`.
fn apply_patch_to_copy(rel: &str, base: &[u8], patch_text: &str) -> Vec<u8> {
    let scratch = tempfile::tempdir().expect("create scratch dir");
    let target = scratch.path().join(rel);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).expect("create scratch parent");
    }
    std::fs::write(&target, base).expect("write scratch base");
    let patch_file = scratch.path().join(".hegel-patch.diff");
    std::fs::write(&patch_file, patch_text).expect("write patch file");
    let output = Command::new("patch")
        .args(["-p1", "-F", "0", "--batch", "--silent", "-i"])
        .arg(&patch_file)
        .current_dir(scratch.path())
        .output()
        .expect("run patch");
    assert!(
        output.status.success(),
        "patch failed for {rel}: {}\n--- patch ---\n{patch_text}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::read(&target).expect("read patched file")
}

/// Build the patch for `section` minus the `exclude`-th hunk, by hand, from
/// jj's own `@@` blocks. Deliberately not the tool's code path.
fn oracle_patch(section: &[u8], exclude: usize) -> String {
    let text = std::str::from_utf8(section).expect("text section is valid UTF-8");
    let mut patch = String::new();
    let mut lines = text.lines();
    for line in lines.by_ref() {
        if line.starts_with("--- ") || line.starts_with("+++ ") {
            patch.push_str(line);
            patch.push('\n');
        }
        if line.starts_with("+++ ") {
            break;
        }
    }
    for (index, (header, body)) in hunk_blocks(section).iter().enumerate() {
        if index == exclude {
            continue;
        }
        patch.push_str(header);
        patch.push('\n');
        for line in body {
            patch.push_str(line);
            patch.push('\n');
        }
    }
    patch
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
