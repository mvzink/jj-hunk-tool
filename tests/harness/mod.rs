//! Shared test harness for the property and coverage suites.
//!
//! Everything here is deliberately independent of the implementation under
//! test: `jj diff --git` output is re-parsed from raw bytes, the tool's own
//! `hunks` output is re-parsed as text, and expected file contents are produced
//! by applying a hand-built patch with `git apply` (and, for patch-safe cases, GNU
//! patch as well). A test that used the tool's parser to check the tool's parser
//! would prove nothing.

// This module is compiled into every property test binary, and each one uses a
// different subset of it.
#![allow(dead_code)]

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use hegel::TestCase;
use hegel::generators as gs;

// ──────────────────────────────────────────────────────────────────────────────
// Test harness
// ──────────────────────────────────────────────────────────────────────────────

pub struct ToolRun {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    /// Coverage markers this invocation reported, via
    /// `JJ_HUNK_TOOL_MARKERS` (see `jj_hunk_tool::markers`).
    pub markers: Vec<String>,
}

impl ToolRun {
    pub fn has_marker(&self, name: &str) -> bool {
        self.markers.iter().any(|marker| marker == name)
    }
}

/// The union of the coverage markers of several tool runs.
///
/// A property that makes more than one call (a listing plus a mutation, say)
/// needs the union: each marker names a path, not a call.
#[derive(Default)]
pub struct Markers(BTreeSet<String>);

impl Markers {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, run: &ToolRun) -> &mut Self {
        for marker in &run.markers {
            self.0.insert(marker.clone());
        }
        self
    }

    /// Add marker names observed outside a tool run (for example by driving
    /// the library in-process).
    pub fn add_names(&mut self, names: &[&str]) -> &mut Self {
        for name in names {
            self.0.insert((*name).to_string());
        }
        self
    }

    pub fn names(&self) -> Vec<String> {
        self.0.iter().cloned().collect()
    }

    /// Assert that `required` was exercised, naming every marker that was not.
    #[track_caller]
    pub fn require(&self, required: &[&str], what: &str) {
        let missing: Vec<&str> = required
            .iter()
            .copied()
            .filter(|marker| !self.0.contains(*marker))
            .collect();
        assert!(
            missing.is_empty(),
            "{what} did not exercise {} coverage marker(s): {}\nreached: {:?}",
            missing.len(),
            missing.join(", "),
            self.names()
        );
    }
}

pub struct Repo {
    pub dir: tempfile::TempDir,
    pub binary: PathBuf,
}

impl Repo {
    pub fn init() -> Self {
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

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn abs(&self, rel: &str) -> PathBuf {
        self.path().join(rel)
    }

    pub fn write(&self, rel: &str, bytes: &[u8]) {
        let path = self.abs(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|e| panic!("create {}: {e}", parent.display()));
        }
        std::fs::write(&path, bytes).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    }

    pub fn remove(&self, rel: &str) {
        let path = self.abs(rel);
        if path.exists() {
            std::fs::remove_file(&path)
                .unwrap_or_else(|e| panic!("remove {}: {e}", path.display()));
        }
    }

    pub fn read(&self, rel: &str) -> Option<Vec<u8>> {
        std::fs::read(self.abs(rel)).ok()
    }

    /// Run `jj`, assert success, return raw stdout bytes (the diff may not be
    /// valid UTF-8 — that is the point).
    pub fn jj(&self, args: &[&str]) -> Vec<u8> {
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
    pub fn jj_file(&self, rev: &str, rel: &str) -> Vec<u8> {
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

    /// Run a `jj` command that is *expected* to fail, returning its stdout and
    /// stderr rather than asserting success.
    pub fn jj_try(&self, args: &[&str]) -> (bool, Vec<u8>, String) {
        let output = Command::new("jj")
            .args(args)
            .arg("--no-pager")
            .current_dir(self.path())
            .env("JJ_CONFIG", "")
            .env("EDITOR", "true")
            .output()
            .expect("run jj");
        (
            output.status.success(),
            output.stdout,
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }

    /// Set a repository-scoped `jj` config value (`jj config set --repo`).
    ///
    /// Used to make a commit immutable (`immutable_heads()`), which is the only
    /// way to reach absorb's "nothing mutable overlaps this hunk" branch.
    pub fn jj_config_repo(&self, key: &str, value: &str) {
        let output = Command::new("jj")
            .args(["config", "set", "--repo", key, value])
            .current_dir(self.path())
            .env("JJ_CONFIG", "")
            .env("EDITOR", "true")
            .output()
            .expect("run jj config set");
        assert!(
            output.status.success(),
            "jj config set --repo {key} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    pub fn tool(&self, args: &[&str]) -> ToolRun {
        self.tool_env(args, &[])
    }

    /// Run the tool with extra environment variables, still collecting coverage
    /// markers. Needed for the internal `_jj-tool` protocol, which is configured
    /// entirely through the environment.
    pub fn tool_env(&self, args: &[&str], env: &[(&str, &str)]) -> ToolRun {
        let mut command = Command::new(&self.binary);
        for (key, value) in env {
            command.env(key, value);
        }
        let output = command
            .args(args)
            .current_dir(self.path())
            .env("JJ_CONFIG", "")
            .env("EDITOR", "true")
            .env("RUST_BACKTRACE", "0")
            // Ask the tool to report which paths it reached; the markers are
            // part of every property's oracle, not a special mode.
            .env(jj_hunk_tool::markers::MARKERS_ENV_VAR, "1")
            .output()
            .expect("run jj-hunk-tool");
        let stdout = String::from_utf8(output.stdout).unwrap_or_else(|e| {
            panic!(
                "jj-hunk-tool {args:?} wrote non-UTF-8 stdout: {e}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let markers = jj_hunk_tool::markers::parse_emitted(&stderr)
            .into_iter()
            .map(str::to_string)
            .collect();
        ToolRun {
            success: output.status.success(),
            stdout,
            stderr,
            markers,
        }
    }

    pub fn tool_ok(&self, args: &[&str]) -> ToolRun {
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
pub fn has_decode_error(stderr: &str) -> bool {
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
pub struct Bytes(pub Vec<u8>);

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
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

/// One file's state in the base commit (`base`) and in the working copy
/// (`head`). `None` means "does not exist", so a spec models additions,
/// modifications, deletions and untouched files alike.
#[derive(Debug, Clone)]
pub struct FileSpec {
    pub path: String,
    pub base: Option<Bytes>,
    pub head: Option<Bytes>,
}

/// A generated repository: a base commit plus working-copy changes.
#[derive(Debug, Clone)]
pub struct Case {
    pub files: Vec<FileSpec>,
    /// Plain-text file that provably changes and whose path is safe for
    /// both appliers (no spaces), so mutation properties can select its hunks.
    pub text: String,
    /// The non-UTF-8 file that is *added*, so its content must appear in the
    /// diff body. Properties assert on this file's own section: see
    /// [`mixed_case`] for why it cannot be swallowed by rename detection.
    pub junk: String,
}

impl Case {
    pub fn text_spec(&self) -> &FileSpec {
        self.files
            .iter()
            .find(|file| file.path == self.text)
            .expect("case always contains its primary text file")
    }

    pub fn paths(&self) -> Vec<String> {
        self.files.iter().map(|file| file.path.clone()).collect()
    }
}

pub fn build_repo(case: &Case) -> Repo {
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

pub const STEM_ALPHABET: &str = "abcdefghijklmnopqrstuvwxyz0123456789 _.-";

pub fn eol(crlf: bool) -> &'static str {
    if crlf { "\r\n" } else { "\n" }
}

/// Plain text: ASCII lines, optionally CRLF, optionally without a trailing
/// newline. Each line is non-empty, so the file itself is never empty: an empty
/// base file cannot be proved to change, and this is used as base content.
///
/// `allow_crlf` exists because CRLF content currently trips a *different*,
/// pre-existing bug: `git-surgeon`'s parser splits the diff with `str::lines()`,
/// which strips the `\r`, so a hunk for a CRLF file is regenerated without its
/// carriage returns and GNU patch refuses it ("different line endings"). That
/// affects `hunks` output, `patch` and every selection command equally on the
/// pre-fix and post-fix code, so the mutation properties below use LF-only text
/// and keep CRLF for the read-only properties, where listing still works.
#[hegel::composite]
pub fn text_content(tc: &TestCase, allow_crlf: bool) -> Bytes {
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
pub fn non_utf8_content(tc: &TestCase) -> Bytes {
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
pub fn junk_blob_content(tc: &TestCase) -> Bytes {
    let lead: u8 = tc.draw(gs::integers::<u8>().min_value(0x80).max_value(0xbf));
    let body: Vec<u8> = tc.draw(gs::vecs(gs::integers::<u8>().min_value(1)).max_size(24));
    let mut bytes = vec![lead];
    bytes.extend_from_slice(&body);
    bytes.push(b'\n');
    Bytes(bytes)
}

/// Anything a working tree may hold, for the extra files of a case.
#[hegel::composite]
pub fn arbitrary_content(tc: &TestCase) -> Bytes {
    if tc.draw(gs::booleans()) {
        tc.draw(text_content(true))
    } else {
        tc.draw(junk_blob_content())
    }
}

/// LF-only text content, including the empty file (see [`text_content`] for why
/// the mutation properties avoid CRLF).
#[hegel::composite]
pub fn arbitrary_lf_text_content(tc: &TestCase) -> Bytes {
    if tc.draw(gs::weighted_booleans(0.9)) {
        tc.draw(text_content(false))
    } else {
        Bytes(Vec::new())
    }
}

/// A path with no spaces, safe to feed to both appliers (GNU patch, the oracle,
/// cannot open a path containing a space). May still be non-ASCII or live in a
/// subdirectory.
#[hegel::composite]
pub fn plain_path(tc: &TestCase) -> String {
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
pub fn awkward_path(tc: &TestCase) -> String {
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
pub enum Mutation {
    Append,
    Prepend,
    ReplaceFirstLine,
    DropFirstLine,
    AppendCrlf,
}

pub fn mutate(base: &Bytes, mutation: Mutation) -> Bytes {
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
pub fn mutation(tc: &TestCase) -> Mutation {
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
pub fn uniquify(path: String, seen: &mut BTreeSet<String>) -> String {
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

// ──────────────────────────────────────────────────────────────────────────────
// Independent oracle: line and section handling of raw `jj diff --git` bytes
// ──────────────────────────────────────────────────────────────────────────────

pub fn line_offsets(bytes: &[u8]) -> Vec<(usize, &[u8])> {
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
pub fn section<'a>(diff: &'a [u8], path: &str) -> Option<&'a [u8]> {
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

/// The diff section for the file at `path`, matched by any of the lines that can
/// name it.
///
/// [`section`] only matches `diff --git a/<path> b/<path>`, which a rename (two
/// different paths), a deletion (no new-side path) or a metadata-only section
/// (no `---`/`+++` at all) does not contain. This matches the `--- a/`, `+++ b/`
/// and `rename from`/`rename to` lines instead, so every shape has exactly one
/// section that can be addressed without the parser's help.
pub fn section_for<'a>(diff: &'a [u8], path: &str) -> Option<&'a [u8]> {
    let needles = [
        format!("diff --git a/{path} b/{path}"),
        format!("--- a/{path}"),
        format!("+++ b/{path}"),
        format!("rename from {path}"),
        format!("rename to {path}"),
        format!("copy from {path}"),
        format!("copy to {path}"),
    ];
    let starts: Vec<usize> = line_offsets(diff)
        .iter()
        .filter(|(_, line)| line.starts_with(b"diff --git "))
        .map(|(offset, _)| *offset)
        .collect();
    for (index, start) in starts.iter().enumerate() {
        let end = starts.get(index + 1).copied().unwrap_or(diff.len());
        let section = &diff[*start..end];
        if line_offsets(section).iter().any(|(_, line)| {
            let line: &[u8] = line;
            needles.iter().any(|needle| line == needle.as_bytes())
        }) {
            return Some(section);
        }
    }
    None
}

pub fn is_hunk_header(line: &[u8]) -> bool {
    line.starts_with(b"@@ -")
}

/// `(header, body lines)` for each `@@` block in a section.
pub fn hunk_blocks(section: &[u8]) -> Vec<(String, Vec<String>)> {
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
pub fn diff_sections(diff: &[u8]) -> Vec<&[u8]> {
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
pub struct Reported {
    pub id: String,
    pub file: String,
    /// Additions/deletions as the `(+A -D)` part of the listing header claims.
    pub adds: usize,
    pub dels: usize,
    pub lines: Vec<String>,
}

/// Parse the `(+A -D)` counts printed after the path in a `hunks` header.
pub fn parse_counts(tail: &str) -> Option<(usize, usize)> {
    let (_, counts) = tail.rsplit_once(" (+")?;
    let counts = counts.strip_suffix(')')?;
    let (adds, dels) = counts.split_once(" -")?;
    Some((adds.parse().ok()?, dels.parse().ok()?))
}

/// Split `hunks` output into per-hunk records.
///
/// A header looks like `<7 hex digits> <path>[ <func context>] (+A -D)`; body
/// lines look like `<n>:<diff line>`. Paths are matched against `known`, the
/// paths this case generated, taking the longest match so a path that is a
/// prefix of another cannot confuse it.
pub fn parse_hunks(output: &str, known: &[String]) -> Vec<Reported> {
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
            Some((id, file)) => {
                let tail = line.split_once(' ').map(|(_, rest)| rest).unwrap_or("");
                let (adds, dels) = parse_counts(tail).unwrap_or((0, 0));
                hunks.push(Reported {
                    id,
                    file,
                    adds,
                    dels,
                    lines: Vec::new(),
                })
            }
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
pub fn count_reported(output: &str) -> usize {
    output
        .lines()
        .filter(|line| {
            line.split_once(' ').is_some_and(|(id, rest)| {
                id.len() == 7 && id.chars().all(|c| c.is_ascii_hexdigit()) && rest.contains(" (+")
            })
        })
        .count()
}

pub fn hunks_for(paths: &[String], output: &str) -> Vec<String> {
    parse_hunks(output, paths)
        .into_iter()
        .map(|hunk| hunk.id)
        .collect()
}

// ──────────────────────────────────────────────────────────────────────────────
// patch appliers: the runtime one and the strictness oracle
// ──────────────────────────────────────────────────────────────────────────────

/// Write `base` at `rel` in a fresh scratch directory, write `patch_text` beside
/// it and hand both to `command`, then return the resulting bytes of `rel`.
///
/// No repository is created and none is needed: `git apply` outside a working
/// tree patches the paths it is given relative to the current directory, which is
/// exactly how the tool's own `_jj-tool` path runs it (jj hands that protocol two
/// repo-less temp directories). A helper that ran `git init` first would test a
/// different code path from the one the tool uses.
fn apply_in_scratch(
    rel: &str,
    base: &[u8],
    patch_text: &str,
    applier: &str,
    command: Command,
    rejected: &str,
) -> Vec<u8> {
    let scratch = tempfile::tempdir().expect("create scratch dir");
    let target = scratch.path().join(rel);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).expect("create scratch parent");
    }
    std::fs::write(&target, base).expect("write scratch base");
    let patch_file = scratch.path().join(".hegel-patch.diff");
    std::fs::write(&patch_file, patch_text).expect("write patch file");

    let mut command = command;
    let output = command
        .arg(&patch_file)
        .current_dir(scratch.path())
        .output()
        .unwrap_or_else(|error| panic!("could not run {applier}: {error}"));
    assert!(
        output.status.success(),
        "{rejected} ({applier}) for {rel}: {}\n--- patch ---\n{patch_text}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::read(&target).expect("read patched file")
}

/// Apply `patch_text` to a scratch copy of `base` with plain `git apply` and
/// return the resulting bytes of `rel`.
///
/// This is the applier the tool now runs (`tool::jj_tool_apply`), so the
/// property suite applies payloads exactly the way the tool does: no repository,
/// no `--unidiff-zero`, no fuzz. `git apply` locates a hunk by content, so it
/// pins the property under test (the payload means what it says) without
/// GNU patch's positional heuristics.
pub fn apply_patch_to_copy(rel: &str, base: &[u8], patch_text: &str) -> Vec<u8> {
    apply_in_scratch(
        rel,
        base,
        patch_text,
        "git apply",
        {
            let mut command = Command::new("git");
            command.args(["apply", "-p1"]);
            command
        },
        "git apply rejected the patch",
    )
}

/// The same payload applied with GNU patch 2.8, its strictness oracle.
///
/// GNU patch is no longer a runtime dependency: it is retained in the test suite
/// because it is stricter than `git apply` about the *shape* of a hunk and the
/// two therefore disagree on exactly the malformations that matter. A payload
/// only one of them can read is not one this tool should print.
///
/// Two of GNU patch's limits make it unusable as an oracle for *every* case, so
/// the callers that use it draw patch-safe inputs: it cannot open a path
/// containing a space (`tests/properties_shapes.rs` draws those, and uses
/// [`apply_patch_to_copy`] alone), and it rejects a payload against a file whose
/// lines end in CRLF because the parser drops the carriage returns before the
/// hunk reaches it (the pre-existing bug documented on `text_content`).
pub fn apply_patch_with_gnu_patch(rel: &str, base: &[u8], patch_text: &str) -> Vec<u8> {
    apply_in_scratch(
        rel,
        base,
        patch_text,
        "GNU patch",
        {
            let mut command = Command::new("patch");
            // `--batch` never asks a question (`patch(1)` prompts on a path it
            // cannot find and would hang the suite), and
            // `--no-backup-if-mismatch` keeps `.orig` files out of the scratch
            // directory so the temp dir is the only record of the result.
            command.args([
                "-p1",
                "--silent",
                "--batch",
                "--no-backup-if-mismatch",
                "-i",
            ]);
            command
        },
        "GNU patch rejected the payload",
    )
}

/// Apply the payload with **both** appliers and require them to agree.
///
/// The differential oracle: `git apply` is what the tool runs and GNU patch is
/// what it used to run, and a payload that survives both is one that does not
/// depend on either one's leniency. See [`apply_patch_with_gnu_patch`] for why
/// callers must draw patch-safe inputs.
pub fn apply_patch_to_copy_with_both(rel: &str, base: &[u8], patch_text: &str) -> Vec<u8> {
    let from_git = apply_patch_to_copy(rel, base, patch_text);
    let from_patch = apply_patch_with_gnu_patch(rel, base, patch_text);
    assert_eq!(
        from_git, from_patch,
        "the two appliers disagree for {rel}: git apply and GNU patch accepted the \
         payload but produced different bytes"
    );
    from_git
}

/// [`apply_patch_to_copy`], applied backwards (`git apply -R`).
///
/// `jj-hunk-tool patch --reverse` builds a *reverse-aware* payload: it keeps the
/// hunk's `+`/`-` markers and changes which side an unselected line falls back to,
/// exactly like `restore` does when it hands the payload to `patch --reverse`.
/// The consumer of such a payload therefore reverses it at apply time, which is
/// what this helper does. (The pre-existing integration test
/// `patch_reverse_whole_hunk` pins the same semantics.)
pub fn apply_patch_reversed_to_copy(rel: &str, base: &[u8], patch_text: &str) -> Vec<u8> {
    apply_in_scratch(
        rel,
        base,
        patch_text,
        "git apply -R",
        {
            let mut command = Command::new("git");
            command.args(["apply", "-R", "-p1"]);
            command
        },
        "git apply -R rejected the reverse patch",
    )
}

/// Build the patch for `section` minus the `exclude`-th hunk, by hand, from
/// jj's own `@@` blocks. Deliberately not the tool's code path.
pub fn oracle_patch(section: &[u8], exclude: usize) -> String {
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

/// The file header (`---`/`+++` lines) of a diff section, as `patch` wants it.
pub fn file_header_lines(section: &[u8]) -> Vec<String> {
    let text = std::str::from_utf8(section).expect("text section is valid UTF-8");
    text.lines()
        .take_while(|line| !line.starts_with("@@ "))
        .filter(|line| line.starts_with("--- ") || line.starts_with("+++ "))
        .map(str::to_string)
        .collect()
}

/// The patch a correct implementation must print for exactly the `picked`-th
/// `@@` blocks of `section`: jj's own file header, then those blocks verbatim.
pub fn oracle_blocks_patch(section: &[u8], picked: &[usize]) -> String {
    let mut patch = String::new();
    for line in file_header_lines(section) {
        patch.push_str(&line);
        patch.push('\n');
    }
    for (index, (header, body)) in hunk_blocks(section).iter().enumerate() {
        if !picked.contains(&index) {
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

/// One `@@` block with its context balanced, as the payload builder emits it.
///
/// An independent re-derivation of the documented rule (`diff::balance_context`):
/// trim the longer context side down to the length of the shorter one but never
/// below one line, renumber the header from the lines that survive, and keep a
/// trailing `\ No newline at end of file` marker only while the line it annotates
/// survives. Written from the rule, not from the implementation, so a payload
/// that agrees with it agrees for the right reason. `jj`'s own blocks are uneven
/// at file boundaries (one context line before a change on line 2, none before a
/// change on line 1), which is why a whole-hunk selection does not come back
/// byte-identical to the listing.
///
/// The one-line floor is load-bearing: trimming a boundary hunk to no context at
/// all would leave a payload GNU patch accepts and `git apply` rejects without
/// `--unidiff-zero`.
pub fn balance_block(header: &str, body: &[String]) -> (String, Vec<String>) {
    let has_marker = body.last().is_some_and(|line| line.starts_with('\\'));
    let core = if has_marker {
        &body[..body.len() - 1]
    } else {
        body
    };
    let is_context = |line: &String| line.starts_with(' ');
    let leading = core.iter().take_while(|line| is_context(line)).count();
    let trailing = core
        .iter()
        .rev()
        .take_while(|line| is_context(line))
        .count();
    if leading == trailing {
        return (header.to_string(), body.to_vec());
    }

    let shorter = leading.min(trailing);
    let keep = if shorter == 0 {
        1.min(leading.max(trailing))
    } else {
        shorter
    };
    let skip = leading.saturating_sub(keep);
    let end = core.len() - trailing.saturating_sub(keep);
    if skip == 0 && end == core.len() {
        return (header.to_string(), body.to_vec());
    }

    let mut lines = core[skip..end].to_vec();
    if has_marker && end == core.len() {
        lines.push(body[body.len() - 1].clone());
    }

    let count = |sign: char| {
        lines
            .iter()
            .filter(|line| line.starts_with(' ') || line.starts_with(sign))
            .count()
    };
    let (old_count, new_count) = (count('-'), count('+'));
    let (old_start, _, new_start, _) = parse_hunk_header(header);
    let function_context = header.splitn(3, "@@").nth(2).unwrap_or("");
    let balanced_header = format!(
        "@@ -{},{} +{},{} @@{}",
        old_start + skip,
        old_count,
        new_start + skip,
        new_count,
        function_context
    );
    (balanced_header, lines)
}

/// [`oracle_blocks_patch`], with each picked block context-balanced by
/// [`balance_block`] — the payload a correct implementation prints.
pub fn oracle_payload(section: &[u8], picked: &[usize]) -> String {
    let mut patch = String::new();
    for line in file_header_lines(section) {
        patch.push_str(&line);
        patch.push('\n');
    }
    for (index, (header, body)) in hunk_blocks(section).iter().enumerate() {
        if !picked.contains(&index) {
            continue;
        }
        let (header, body) = balance_block(header, body);
        patch.push_str(&header);
        patch.push('\n');
        for line in &body {
            patch.push_str(line);
            patch.push('\n');
        }
    }
    patch
}

/// `(old_start, old_count, new_start, new_count)` from a `@@` header.
pub fn parse_hunk_header(header: &str) -> (usize, usize, usize, usize) {
    let mut parts = header
        .trim_start_matches("@@ ")
        .split(" @@")
        .next()
        .expect("header has a terminating @@")
        .split_whitespace();
    let old = parts.next().expect("old range");
    let new = parts.next().expect("new range");
    let range = |spec: &str| -> (usize, usize) {
        let spec = &spec[1..];
        match spec.split_once(',') {
            Some((start, count)) => (
                start.parse().expect("old start"),
                count.parse().expect("count"),
            ),
            None => (spec.parse().expect("start"), 1),
        }
    };
    let (old_start, old_count) = range(old);
    let (new_start, new_count) = range(new);
    (old_start, old_count, new_start, new_count)
}

/// The old-side and new-side lines a hunk body claims to cover.
///
/// `\ No newline at end of file` markers are metadata, not content lines.
pub fn body_sides(body: &[String]) -> (Vec<String>, Vec<String>) {
    let old = body
        .iter()
        .filter(|line| !line.starts_with('+') && !line.starts_with('\\'))
        .map(|line| {
            line.strip_prefix(' ')
                .or_else(|| line.strip_prefix('-'))
                .unwrap_or(line)
                .to_string()
        })
        .collect();
    let new = body
        .iter()
        .filter(|line| !line.starts_with('-') && !line.starts_with('\\'))
        .map(|line| {
            line.strip_prefix(' ')
                .or_else(|| line.strip_prefix('+'))
                .unwrap_or(line)
                .to_string()
        })
        .collect();
    (old, new)
}

// ──────────────────────────────────────────────────────────────────────────────
// Independent oracle: hunk ids
// ──────────────────────────────────────────────────────────────────────────────

/// SHA-1, implemented here rather than taken from a crate so that the id oracle
/// does not share code (or a dependency) with the implementation it checks.
pub fn sha1(bytes: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut message = bytes.to_vec();
    let bit_len = (bytes.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in message.as_chunks::<64>().0 {
        let mut w = [0u32; 80];
        for (i, word) in chunk.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, word) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// The id the tool must assign to a hunk with this display path and body,
/// before collision suffixes: the first seven hex digits of the SHA-1 of the
/// path followed by every body line and a newline.
pub fn expected_raw_id(file: &str, body: &[String]) -> String {
    let mut material = file.as_bytes().to_vec();
    for line in body {
        material.extend_from_slice(line.as_bytes());
        material.push(b'\n');
    }
    let digest = sha1(&material);
    digest[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()[..7]
        .to_string()
}

/// Assign ids the way the tool must: [`expected_raw_id`], with the second and
/// later hunks that share a prefix suffixed `-2`, `-3`, … in listing order.
pub fn expected_ids(hunks: &[(String, Vec<String>)]) -> Vec<String> {
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    hunks
        .iter()
        .map(|(file, body)| {
            let raw = expected_raw_id(file, body);
            let count = seen.entry(raw.clone()).or_insert(0);
            *count += 1;
            if *count == 1 {
                raw
            } else {
                format!("{raw}-{count}")
            }
        })
        .collect()
}
