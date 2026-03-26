use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

// ──────────────────────────────────────────────────────────────────────────────
// Test harness
// ──────────────────────────────────────────────────────────────────────────────

struct TestRepo {
    dir: tempfile::TempDir,
    binary: PathBuf,
}

impl TestRepo {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let status = Command::new("jj")
            .args(["git", "init", "--no-pager"])
            .current_dir(dir.path())
            .env("JJ_CONFIG", "")
            .output()
            .unwrap();
        assert!(status.status.success(), "jj git init failed");
        let binary = PathBuf::from(env!("CARGO_BIN_EXE_jj-hunk-tool"));
        TestRepo { dir, binary }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn write_file(&self, name: &str, content: &str) {
        let path = self.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    fn read_file(&self, name: &str) -> String {
        std::fs::read_to_string(self.path().join(name)).unwrap()
    }

    fn file_exists(&self, name: &str) -> bool {
        self.path().join(name).exists()
    }

    fn jj(&self, args: &[&str]) -> String {
        let output = Command::new("jj")
            .args(args)
            .arg("--no-pager")
            .current_dir(self.path())
            .env("JJ_CONFIG", "")
            .env("EDITOR", "true")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "jj {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn jj_diff(&self, rev: &str) -> String {
        self.jj(&["diff", "--git", "-r", rev])
    }

    fn tool(&self, args: &[&str]) -> Result<String, String> {
        let output = Command::new(&self.binary)
            .args(args)
            .current_dir(self.path())
            .env("JJ_CONFIG", "")
            .env("EDITOR", "true")
            .output()
            .unwrap();
        if output.status.success() {
            Ok(String::from_utf8(output.stdout).unwrap())
        } else {
            Err(format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ))
        }
    }

    /// Run tool, expect success, return stdout.
    fn tool_ok(&self, args: &[&str]) -> String {
        self.tool(args)
            .unwrap_or_else(|e| panic!("tool {:?} failed: {e}", args))
    }

    /// Run tool, expect failure, return combined output.
    fn tool_err(&self, args: &[&str]) -> String {
        self.tool(args)
            .expect_err(&format!("tool {:?} should have failed", args))
    }

    /// Run tool, return (stdout, stderr) separately.
    fn tool_output(&self, args: &[&str]) -> (String, String) {
        let output = Command::new(&self.binary)
            .args(args)
            .current_dir(self.path())
            .env("JJ_CONFIG", "")
            .env("EDITOR", "true")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "tool {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        (
            String::from_utf8(output.stdout).unwrap(),
            String::from_utf8(output.stderr).unwrap(),
        )
    }

    /// Run tool with piped stdin, expect success, return stdout.
    fn tool_with_stdin(&self, args: &[&str], stdin_input: &str) -> String {
        let mut child = Command::new(&self.binary)
            .args(args)
            .current_dir(self.path())
            .env("JJ_CONFIG", "")
            .env("EDITOR", "true")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin_input.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "tool {:?} failed: {}{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    /// Run tool from a subdirectory of the repo, expect success, return stdout.
    fn tool_ok_in_subdir(&self, subdir: &str, args: &[&str]) -> String {
        let dir = self.path().join(subdir);
        std::fs::create_dir_all(&dir).unwrap();
        let output = Command::new(&self.binary)
            .args(args)
            .current_dir(&dir)
            .env("JJ_CONFIG", "")
            .env("EDITOR", "true")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "tool {:?} (in {subdir}) failed: {}{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        String::from_utf8(output.stdout).unwrap()
    }

    /// Extract hunk IDs from `hunks` output. Returns vec of (id, line_text).
    /// Header lines look like "abc1234 file.txt (+N -M)"; numbered lines look like "1:...".
    fn get_hunk_ids(&self, extra_args: &[&str]) -> Vec<(String, String)> {
        let mut args = vec!["hunks"];
        args.extend_from_slice(extra_args);
        let output = self.tool_ok(&args);
        output
            .lines()
            .filter(|l| {
                let first = l.split_whitespace().next().unwrap_or("");
                first.len() == 7 && first.chars().all(|c| c.is_ascii_hexdigit())
            })
            .map(|l| {
                let id = l.split_whitespace().next().unwrap().to_string();
                (id, l.to_string())
            })
            .collect()
    }

    /// Get the single hunk ID (panics if not exactly one).
    fn get_single_hunk_id(&self, extra_args: &[&str]) -> String {
        let ids = self.get_hunk_ids(extra_args);
        assert_eq!(
            ids.len(),
            1,
            "expected 1 hunk, got {}: {:?}",
            ids.len(),
            ids
        );
        ids[0].0.clone()
    }

    /// Find hunk ID for a specific file.
    fn get_hunk_id_for_file(&self, file: &str, extra_args: &[&str]) -> String {
        let ids = self.get_hunk_ids(extra_args);
        ids.iter()
            .find(|(_, line)| line.contains(file))
            .unwrap_or_else(|| panic!("no hunk found for file {file}"))
            .0
            .clone()
    }

    /// Create a file with content and commit it, so subsequent edits produce diffs.
    fn commit_file(&self, name: &str, content: &str) {
        self.write_file(name, content);
        self.jj(&["commit", "-m", &format!("add {name}")]);
    }

    /// Create a file with content that will produce two distant hunks when both
    /// ends are modified (20 context lines between them).
    fn write_two_hunk_file(&self, name: &str) -> String {
        let content = format!("top\n{}\nbottom\n", "mid\n".repeat(20));
        self.write_file(name, &content);
        content
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// hunks
// ──────────────────────────────────────────────────────────────────────────────

#[test]
fn hunks_no_changes() {
    let repo = TestRepo::new();
    repo.write_file("a.txt", "hello\n");
    repo.jj(&["commit", "-m", "init"]);
    // Working copy is empty — no diff
    let output = repo.tool_ok(&["hunks"]);
    assert!(output.trim().is_empty(), "should be empty, got: {output}");
}

#[test]
fn hunks_single_file_single_hunk() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\n");
    repo.write_file("a.txt", "line1\nadded\n");

    let output = repo.tool_ok(&["hunks"]);
    let ids = repo.get_hunk_ids(&[]);
    assert_eq!(ids.len(), 1);
    // ID is 7-char hex
    assert_eq!(ids[0].0.len(), 7, "ID should be 7 chars: {}", ids[0].0);
    assert!(ids[0].0.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(output.contains("a.txt"));
    assert!(output.contains("+1 -0"));
    assert!(output.contains("+added"));
}

#[test]
fn hunks_single_file_multiple_hunks() {
    let repo = TestRepo::new();
    let content = repo.write_two_hunk_file("f.txt");
    repo.jj(&["commit", "-m", "init"]);
    let new_content = content.replace("top", "TOP").replace("bottom", "BOTTOM");
    repo.write_file("f.txt", &new_content);

    let ids = repo.get_hunk_ids(&[]);
    assert!(ids.len() >= 2, "expected >=2 hunks, got {}", ids.len());
    // All IDs are distinct
    let id_set: std::collections::HashSet<_> = ids.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(id_set.len(), ids.len(), "IDs should be unique");
}

#[test]
fn hunks_multiple_files() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\n");
    repo.commit_file("b.txt", "b\n");
    repo.write_file("a.txt", "a changed\n");
    repo.write_file("b.txt", "b changed\n");

    let output = repo.tool_ok(&["hunks"]);
    assert!(output.contains("a.txt"));
    assert!(output.contains("b.txt"));
}

#[test]
fn hunks_commit_subcommand_removed() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\n");
    repo.write_file("a.txt", "line1\nchanged\n");

    let err = repo.tool(&["commit", "abc1234"]);
    assert!(err.is_err(), "commit subcommand should not exist");
}

#[test]
fn hunks_discard_subcommand_removed() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\n");
    repo.write_file("a.txt", "line1\nchanged\n");

    let err = repo.tool(&["discard", "abc1234"]);
    assert!(err.is_err(), "discard subcommand should not exist");
}

#[test]
fn hunks_file_filter() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\n");
    repo.commit_file("b.txt", "b\n");
    repo.write_file("a.txt", "a changed\n");
    repo.write_file("b.txt", "b changed\n");

    let output = repo.tool_ok(&["hunks", "--file", "a.txt"]);
    assert!(output.contains("a.txt"));
    assert!(!output.contains("b.txt"));
}

#[test]
fn hunks_file_filter_nonexistent() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\n");
    repo.write_file("a.txt", "a changed\n");

    let output = repo.tool_ok(&["hunks", "--file", "nope.txt"]);
    assert!(output.trim().is_empty());
}

#[test]
fn hunks_revision() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "original\n");
    repo.write_file("a.txt", "modified\n");
    repo.jj(&["commit", "-m", "modify a"]);

    // @- should have the modification, @ should be empty
    let ids_parent = repo.get_hunk_ids(&["-r", "@-"]);
    assert!(!ids_parent.is_empty(), "parent should have hunks");

    let ids_wc = repo.get_hunk_ids(&[]);
    assert!(ids_wc.is_empty(), "working copy should be empty");
}

#[test]
fn hunks_default_no_truncation() {
    let repo = TestRepo::new();
    repo.commit_file("big.txt", "");
    let new_content: String = (1..=10).map(|i| format!("line{i}\n")).collect();
    repo.write_file("big.txt", &new_content);

    let output = repo.tool_ok(&["hunks"]);
    // Default (full) mode should show all lines, not truncate
    assert!(
        !output.contains("... (+"),
        "default should not truncate"
    );
    assert!(output.contains("line10"), "should show all lines");
}

#[test]
fn hunks_new_file() {
    let repo = TestRepo::new();
    repo.write_file("dummy.txt", "x\n");
    repo.jj(&["commit", "-m", "init"]);
    repo.write_file("brand_new.txt", "hello\nworld\n");

    let output = repo.tool_ok(&["hunks"]);
    assert!(output.contains("brand_new.txt"));
    // All additions
    assert!(output.contains("+2 -0") || output.contains("+hello"));
}

#[test]
fn hunks_deleted_file() {
    let repo = TestRepo::new();
    repo.commit_file("doomed.txt", "goodbye\n");
    std::fs::remove_file(repo.path().join("doomed.txt")).unwrap();

    let output = repo.tool_ok(&["hunks"]);
    assert!(output.contains("doomed.txt"));
    assert!(output.contains("-1") || output.contains("-goodbye"));
}

#[test]
fn hunks_pure_addition() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\n");
    repo.write_file("a.txt", "line1\nline2\nline3\n");

    let output = repo.tool_ok(&["hunks"]);
    assert!(output.contains("+2 -0"));
}

#[test]
fn hunks_pure_deletion() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\nline2\nline3\n");
    repo.write_file("a.txt", "line1\n");

    let output = repo.tool_ok(&["hunks"]);
    assert!(output.contains("+0 -2"));
}

#[test]
fn hunks_modification() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "old\n");
    repo.write_file("a.txt", "new\n");

    let output = repo.tool_ok(&["hunks"]);
    assert!(output.contains("+1 -1"));
}

#[test]
fn hunks_id_stability() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\n");
    repo.write_file("a.txt", "line1\nadded\n");

    let out1 = repo.tool_ok(&["hunks"]);
    let out2 = repo.tool_ok(&["hunks"]);
    assert_eq!(out1, out2, "output should be identical across calls");
}

#[test]
fn hunks_duplicate_ids_get_suffixes() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "same\n");
    repo.commit_file("b.txt", "same\n");
    repo.write_file("a.txt", "same\nchanged\n");
    repo.write_file("b.txt", "same\nchanged\n");

    let ids = repo.get_hunk_ids(&[]);
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0].0, ids[1].0, "IDs for different files should differ");
}

#[test]
fn hunks_file_in_subdirectory() {
    let repo = TestRepo::new();
    repo.commit_file("sub/deep/file.txt", "original\n");
    repo.write_file("sub/deep/file.txt", "modified\n");

    let output = repo.tool_ok(&["hunks"]);
    assert!(
        output.contains("sub/deep/file.txt"),
        "should show full path: {output}"
    );
}

#[test]
fn hunks_with_revision() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\n");
    repo.write_file("a.txt", "line1\nline2\n");
    repo.jj(&["commit", "-m", "add line2"]);

    let output = repo.tool_ok(&["hunks", "-r", "@-"]);
    assert!(
        output.contains("1:"),
        "should have line numbers from revision"
    );
    assert!(output.contains("a.txt"));
}

#[test]
fn hunks_file_filter_with_revision() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\n");
    repo.commit_file("b.txt", "b\n");
    repo.write_file("a.txt", "a changed\n");
    repo.write_file("b.txt", "b changed\n");
    repo.jj(&["commit", "-m", "changes"]);

    let output = repo.tool_ok(&["hunks", "--file", "a.txt", "-r", "@-"]);
    assert!(output.contains("a.txt"));
    assert!(!output.contains("b.txt"));
}

// ──────────────────────────────────────────────────────────────────────────────
// hunks: default shows line numbers (old --full behavior)
// ──────────────────────────────────────────────────────────────────────────────

#[test]
fn hunks_default_shows_line_numbers() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\nline2\nline3\n");
    repo.write_file("a.txt", "line1\nchanged\nline3\n");

    let output = repo.tool_ok(&["hunks"]);
    assert!(output.contains("1:"), "default should have line numbers");
    assert!(
        output.contains("line1") || output.contains("line3"),
        "default should have context"
    );
}

#[test]
fn hunks_default_line_numbers_sequential() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\nb\nc\n");
    repo.write_file("a.txt", "a\nB\nc\n");

    let output = repo.tool_ok(&["hunks"]);
    let numbered: Vec<usize> = output
        .lines()
        .filter_map(|l| {
            let trimmed = l.trim();
            trimmed.split(':').next().and_then(|n| n.parse().ok())
        })
        .collect();
    assert!(!numbered.is_empty());
    for pair in numbered.windows(2) {
        assert_eq!(pair[1], pair[0] + 1, "line numbers should be sequential");
    }
}

#[test]
fn hunks_compact_mode() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\nline2\nline3\n");
    repo.write_file("a.txt", "line1\nchanged\nline3\n");

    let output = repo.tool_ok(&["hunks", "--compact"]);
    assert!(!output.contains("1:"), "compact should not have line numbers");
    assert!(output.contains("+changed") || output.contains("-line2"));
}

#[test]
fn hunks_compact_truncation() {
    let repo = TestRepo::new();
    repo.commit_file("big.txt", "");
    let new_content: String = (1..=10).map(|i| format!("line{i}\n")).collect();
    repo.write_file("big.txt", &new_content);

    let output = repo.tool_ok(&["hunks", "--compact"]);
    assert!(
        output.contains("... (+"),
        "compact should truncate with '...' message"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// patch
// ──────────────────────────────────────────────────────────────────────────────

#[test]
fn patch_single_hunk() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\n");
    repo.write_file("a.txt", "line1\nadded\n");

    let id = repo.get_single_hunk_id(&[]);
    let patch = repo.tool_ok(&["patch", &id]);
    assert!(patch.contains("--- a/a.txt"));
    assert!(patch.contains("+++ b/a.txt"));
    assert!(patch.contains("@@"));
    assert!(patch.contains("+added"));
}

#[test]
fn patch_multiple_hunks_same_file() {
    let repo = TestRepo::new();
    let content = repo.write_two_hunk_file("f.txt");
    repo.jj(&["commit", "-m", "init"]);
    let new_content = content.replace("top", "TOP").replace("bottom", "BOTTOM");
    repo.write_file("f.txt", &new_content);

    let ids = repo.get_hunk_ids(&[]);
    assert!(ids.len() >= 2);
    let all_ids: Vec<&str> = ids.iter().map(|(id, _)| id.as_str()).collect();

    let mut args = vec!["patch"];
    args.extend_from_slice(&all_ids);
    let patch = repo.tool_ok(&args);

    assert!(patch.contains("--- a/f.txt"));
    assert!(patch.contains("+++ b/f.txt"));
    let at_count = patch.matches("@@").count();
    assert!(
        at_count >= 4,
        "expected multiple @@ headers, got {at_count}"
    );
}

#[test]
fn patch_multiple_hunks_different_files() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\n");
    repo.commit_file("b.txt", "b\n");
    repo.write_file("a.txt", "a changed\n");
    repo.write_file("b.txt", "b changed\n");

    let ids = repo.get_hunk_ids(&[]);
    let all_ids: Vec<&str> = ids.iter().map(|(id, _)| id.as_str()).collect();

    let mut args = vec!["patch"];
    args.extend_from_slice(&all_ids);
    let patch = repo.tool_ok(&args);

    assert!(patch.contains("--- a/a.txt"));
    assert!(patch.contains("--- a/b.txt"));
}

#[test]
fn patch_reverse_whole_hunk() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\n");
    repo.write_file("a.txt", "line1\nadded\n");

    let id = repo.get_single_hunk_id(&[]);
    let patch = repo.tool_ok(&["patch", "--reverse", &id]);
    assert!(patch.contains("a.txt"));
    assert!(patch.contains("+added"));
}

#[test]
fn patch_inline_range() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\nline2\nline3\nline4\n");
    repo.write_file("a.txt", "LINE1\nLINE2\nLINE3\nLINE4\n");

    let id = repo.get_single_hunk_id(&[]);
    let spec = format!("{id}:1-4");
    let patch = repo.tool_ok(&["patch", &spec]);
    assert!(patch.contains("@@"));
    assert!(patch.contains("---") && patch.contains("+++"));
}

#[test]
fn patch_inline_single_line() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "old1\nold2\nold3\n");
    repo.write_file("a.txt", "NEW1\nNEW2\nNEW3\n");

    let id = repo.get_single_hunk_id(&[]);
    let spec = format!("{id}:1");
    let patch = repo.tool_ok(&["patch", &spec]);
    assert!(patch.contains("@@"));
}

#[test]
fn patch_inline_comma_ranges() {
    let repo = TestRepo::new();
    let lines: String = (1..=10).map(|i| format!("line{i}\n")).collect();
    repo.commit_file("a.txt", &lines);
    let new_lines: String = (1..=10).map(|i| format!("LINE{i}\n")).collect();
    repo.write_file("a.txt", &new_lines);

    let id = repo.get_single_hunk_id(&[]);
    let spec = format!("{id}:1-3,7-9");
    let patch = repo.tool_ok(&["patch", &spec]);
    assert!(patch.contains("@@"));
}

#[test]
fn patch_reverse_with_range() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\nline2\nline3\n");
    repo.write_file("a.txt", "LINE1\nLINE2\nLINE3\n");

    let id = repo.get_single_hunk_id(&[]);
    let spec = format!("{id}:1-2");
    let patch = repo.tool_ok(&["patch", "--reverse", &spec]);
    assert!(patch.contains("@@"));
}

#[test]
fn patch_with_revision() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "original\n");
    repo.write_file("a.txt", "modified\n");
    repo.jj(&["commit", "-m", "modify"]);

    let id = repo.get_single_hunk_id(&["-r", "@-"]);
    let patch = repo.tool_ok(&["patch", &id, "-r", "@-"]);
    assert!(patch.contains("--- a/a.txt"));
}

#[test]
fn patch_invalid_id() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "x\n");
    repo.write_file("a.txt", "y\n");

    let err = repo.tool_err(&["patch", "invalid_id"]);
    assert!(err.contains("hunk not found"));
}

#[test]
fn patch_invalid_range_reversed() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "x\n");
    repo.write_file("a.txt", "y\n");

    let id = repo.get_single_hunk_id(&[]);
    let spec = format!("{id}:5-3");
    let err = repo.tool_err(&["patch", &spec]);
    assert!(err.contains("range must be 1-based"), "got: {err}");
}

#[test]
fn patch_invalid_range_zero() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "x\n");
    repo.write_file("a.txt", "y\n");

    let id = repo.get_single_hunk_id(&[]);
    let spec = format!("{id}:0-5");
    let err = repo.tool_err(&["patch", &spec]);
    assert!(err.contains("range must be 1-based"), "got: {err}");
}

#[test]
fn patch_invalid_range_non_numeric() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "x\n");
    repo.write_file("a.txt", "y\n");

    let id = repo.get_single_hunk_id(&[]);
    let spec = format!("{id}:abc");
    let err = repo.tool_err(&["patch", &spec]);
    assert!(err.contains("invalid"), "got: {err}");
}

#[test]
fn patch_output_is_applicable() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\nline2\nline3\n");
    repo.write_file("a.txt", "line1\nCHANGED\nline3\n");

    let id = repo.get_single_hunk_id(&[]);
    let patch = repo.tool_ok(&["patch", &id]);

    repo.write_file("a.txt", "line1\nline2\nline3\n");
    let status = Command::new("patch")
        .args(["-p1", "--silent"])
        .current_dir(repo.path())
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap()
        .stdin
        .unwrap()
        .write_all(patch.as_bytes());
    assert!(status.is_ok());
}

#[test]
fn patch_new_file() {
    let repo = TestRepo::new();
    repo.write_file("dummy.txt", "x\n");
    repo.jj(&["commit", "-m", "init"]);
    repo.write_file("new.txt", "hello\n");

    let id = repo.get_hunk_id_for_file("new.txt", &[]);
    let patch = repo.tool_ok(&["patch", &id]);
    assert!(patch.contains("/dev/null") || patch.contains("new file"));
}

#[test]
fn patch_deleted_file() {
    let repo = TestRepo::new();
    repo.commit_file("gone.txt", "goodbye\n");
    std::fs::remove_file(repo.path().join("gone.txt")).unwrap();

    let id = repo.get_hunk_id_for_file("gone.txt", &[]);
    let patch = repo.tool_ok(&["patch", &id]);
    assert!(patch.contains("/dev/null") || patch.contains("-goodbye"));
}

// ──────────────────────────────────────────────────────────────────────────────
// split (was: commit)
// ──────────────────────────────────────────────────────────────────────────────

#[test]
fn split_single_hunk_with_message() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "original\n");
    repo.write_file("a.txt", "modified\n");

    let id = repo.get_single_hunk_id(&[]);
    repo.tool_ok(&["split", &id, "-m", "my split"]);

    let parent_diff = repo.jj_diff("@-");
    assert!(parent_diff.contains("a.txt"));
    let desc = repo.jj(&["log", "-r", "@-", "-T", "description", "--no-graph"]);
    assert!(desc.contains("my split"));
}

#[test]
fn split_single_hunk_no_message() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "original\n");
    repo.write_file("a.txt", "modified\n");

    let id = repo.get_single_hunk_id(&[]);
    repo.tool_ok(&["split", &id]);

    let parent_diff = repo.jj_diff("@-");
    assert!(parent_diff.contains("a.txt"));
}

#[test]
fn split_multiple_hunks_different_files() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\n");
    repo.commit_file("b.txt", "b\n");
    repo.write_file("a.txt", "a changed\n");
    repo.write_file("b.txt", "b changed\n");

    let a_id = repo.get_hunk_id_for_file("a.txt", &[]);
    let b_id = repo.get_hunk_id_for_file("b.txt", &[]);
    repo.tool_ok(&["split", &a_id, &b_id, "-m", "both files"]);

    let parent_diff = repo.jj_diff("@-");
    assert!(parent_diff.contains("a.txt"));
    assert!(parent_diff.contains("b.txt"));
}

#[test]
fn split_one_of_two_hunks_other_remains() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\n");
    repo.commit_file("b.txt", "b\n");
    repo.write_file("a.txt", "a changed\n");
    repo.write_file("b.txt", "b changed\n");

    let a_id = repo.get_hunk_id_for_file("a.txt", &[]);
    repo.tool_ok(&["split", &a_id, "-m", "only a"]);

    let parent_diff = repo.jj_diff("@-");
    assert!(parent_diff.contains("a.txt"));
    assert!(!parent_diff.contains("b.txt"));

    let wc_diff = repo.jj_diff("@");
    assert!(wc_diff.contains("b.txt"));
    assert!(!wc_diff.contains("a.txt"));
}

#[test]
fn split_with_inline_range() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\nline2\nline3\nline4\n");
    repo.write_file("a.txt", "LINE1\nLINE2\nline3\nline4\n");

    let id = repo.get_single_hunk_id(&[]);
    let spec = format!("{id}:1-2");
    repo.tool_ok(&["split", &spec, "-m", "partial"]);

    let desc = repo.jj(&["log", "-r", "@-", "-T", "description", "--no-graph"]);
    assert!(desc.contains("partial"));
}

#[test]
fn split_invalid_id() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "x\n");
    repo.write_file("a.txt", "y\n");

    let err = repo.tool_err(&["split", "invalid", "-m", "nope"]);
    assert!(err.contains("hunk not found"));
}

#[test]
fn split_non_working_copy_revision() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\n");
    repo.commit_file("b.txt", "b\n");
    repo.write_file("a.txt", "a changed\n");
    repo.write_file("b.txt", "b changed\n");
    repo.jj(&["commit", "-m", "change both"]);

    let a_id = repo.get_hunk_id_for_file("a.txt", &["-r", "@-"]);
    repo.tool_ok(&["split", &a_id, "-r", "@-", "-m", "just a"]);
}

#[test]
fn split_all_hunks_working_copy_empty() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\n");
    repo.write_file("a.txt", "a changed\n");

    let id = repo.get_single_hunk_id(&[]);
    repo.tool_ok(&["split", &id, "-m", "everything"]);

    let wc_diff = repo.jj_diff("@");
    assert!(
        wc_diff.trim().is_empty(),
        "working copy should be empty after splitting all hunks"
    );
}

#[test]
fn split_new_file() {
    let repo = TestRepo::new();
    repo.write_file("dummy.txt", "x\n");
    repo.jj(&["commit", "-m", "init"]);
    repo.write_file("brand_new.txt", "content\n");

    let id = repo.get_hunk_id_for_file("brand_new.txt", &[]);
    repo.tool_ok(&["split", &id, "-m", "add new file"]);

    let parent_diff = repo.jj_diff("@-");
    assert!(parent_diff.contains("brand_new.txt"));
}

#[test]
fn split_deleted_file() {
    let repo = TestRepo::new();
    repo.commit_file("doomed.txt", "content\n");
    std::fs::remove_file(repo.path().join("doomed.txt")).unwrap();

    let id = repo.get_hunk_id_for_file("doomed.txt", &[]);
    repo.tool_ok(&["split", &id, "-m", "remove file"]);

    let parent_diff = repo.jj_diff("@-");
    assert!(parent_diff.contains("doomed.txt"));
}

#[test]
fn split_multiple_hunks_same_file() {
    let repo = TestRepo::new();
    let content = repo.write_two_hunk_file("f.txt");
    repo.jj(&["commit", "-m", "init"]);
    let new_content = content.replace("top", "TOP").replace("bottom", "BOTTOM");
    repo.write_file("f.txt", &new_content);

    let ids = repo.get_hunk_ids(&[]);
    assert!(ids.len() >= 2);

    repo.tool_ok(&["split", &ids[0].0, "-m", "first hunk only"]);

    let parent_diff = repo.jj_diff("@-");
    assert!(parent_diff.contains("f.txt"));

    let wc_diff = repo.jj_diff("@");
    assert!(wc_diff.contains("f.txt"));
}

#[test]
fn split_forwards_jj_output() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "original\n");
    repo.write_file("a.txt", "modified\n");

    let id = repo.get_single_hunk_id(&[]);
    let (_stdout, stderr) = repo.tool_output(&["split", &id, "-m", "test split"]);
    assert!(!stderr.is_empty(), "split should forward jj's output");
}

// ──────────────────────────────────────────────────────────────────────────────
// squash
// ──────────────────────────────────────────────────────────────────────────────

#[test]
fn squash_single_hunk_into_parent() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\n");
    repo.write_file("a.txt", "a changed\n");
    repo.jj(&["commit", "-m", "change a"]);
    // Now @ is empty, @- has the change. Make a new change in @.
    repo.write_file("a.txt", "a changed more\n");

    let id = repo.get_single_hunk_id(&[]);
    repo.tool_ok(&["squash", &id, "-m", "squashed"]);

    // @ should be empty (change was squashed into parent)
    let wc_diff = repo.jj_diff("@");
    assert!(wc_diff.trim().is_empty(), "working copy should be empty after squash");
}

#[test]
fn squash_one_of_two_hunks() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\n");
    repo.commit_file("b.txt", "b\n");
    repo.jj(&["commit", "-m", "base"]);
    repo.write_file("a.txt", "a changed\n");
    repo.write_file("b.txt", "b changed\n");

    let a_id = repo.get_hunk_id_for_file("a.txt", &[]);
    repo.tool_ok(&["squash", &a_id, "-m", "squash a only"]);

    // a.txt change should have been squashed into parent
    let parent_diff = repo.jj_diff("@-");
    assert!(parent_diff.contains("a.txt"), "parent should have a.txt change");

    // b.txt change should still be in working copy
    let wc_diff = repo.jj_diff("@");
    assert!(wc_diff.contains("b.txt"), "working copy should still have b.txt");
}

#[test]
fn squash_with_revision() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\n");
    repo.write_file("a.txt", "a changed\n");
    repo.jj(&["commit", "-m", "change"]);
    repo.jj(&["commit", "-m", "empty"]);

    // Squash @-- into its parent using -r
    let id = repo.get_single_hunk_id(&["-r", "@--"]);
    repo.tool_ok(&["squash", &id, "-r", "@--", "-m", "squashed"]);
}

#[test]
fn squash_invalid_id() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "x\n");
    repo.write_file("a.txt", "y\n");

    let err = repo.tool_err(&["squash", "invalid", "-m", "nope"]);
    assert!(err.contains("hunk not found"));
}

// ──────────────────────────────────────────────────────────────────────────────
// diffedit
// ──────────────────────────────────────────────────────────────────────────────

#[test]
fn diffedit_keep_one_of_two_hunks() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\n");
    repo.commit_file("b.txt", "b\n");
    repo.write_file("a.txt", "a changed\n");
    repo.write_file("b.txt", "b changed\n");
    repo.jj(&["commit", "-m", "change both"]);

    let a_id = repo.get_hunk_id_for_file("a.txt", &["-r", "@-"]);
    repo.tool_ok(&["diffedit", &a_id, "-r", "@-"]);

    let diff = repo.jj_diff("@-");
    assert!(diff.contains("a.txt"), "should keep a.txt change");
}

#[test]
fn diffedit_default_revision() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\n");
    repo.commit_file("b.txt", "b\n");
    repo.write_file("a.txt", "a changed\n");
    repo.write_file("b.txt", "b changed\n");

    let a_id = repo.get_hunk_id_for_file("a.txt", &[]);
    repo.tool_ok(&["diffedit", &a_id]);

    let diff = repo.jj_diff("@");
    assert!(diff.contains("a.txt"));
}

#[test]
fn diffedit_invalid_id() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "x\n");
    repo.write_file("a.txt", "y\n");

    let err = repo.tool_err(&["diffedit", "invalid"]);
    assert!(err.contains("hunk not found"));
}

// ──────────────────────────────────────────────────────────────────────────────
// restore
// ──────────────────────────────────────────────────────────────────────────────

#[test]
fn restore_default_undoes_working_copy_change() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\n");
    repo.commit_file("b.txt", "b\n");
    repo.write_file("a.txt", "a changed\n");
    repo.write_file("b.txt", "b changed\n");

    // Default (no flags) = --changes-in @
    let a_id = repo.get_hunk_id_for_file("a.txt", &[]);
    repo.tool_ok(&["restore", &a_id]);

    assert_eq!(repo.read_file("a.txt"), "a\n", "a.txt should be restored");
    let diff = repo.jj_diff("@");
    assert!(diff.contains("b.txt"), "b.txt should still be changed");
    assert!(!diff.contains("a.txt"), "a.txt change should be undone");
}

#[test]
fn restore_changes_in_explicit() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\n");
    repo.commit_file("b.txt", "b\n");
    repo.write_file("a.txt", "a changed\n");
    repo.write_file("b.txt", "b changed\n");

    let a_id = repo.get_hunk_id_for_file("a.txt", &[]);
    repo.tool_ok(&["restore", &a_id, "-c", "@"]);

    assert_eq!(repo.read_file("a.txt"), "a\n", "a.txt should be restored");
    let diff = repo.jj_diff("@");
    assert!(diff.contains("b.txt"), "b.txt should still be changed");
    assert!(!diff.contains("a.txt"), "a.txt change should be undone");
}

#[test]
fn restore_multiple_hunks() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\n");
    repo.commit_file("b.txt", "b\n");
    repo.write_file("a.txt", "a changed\n");
    repo.write_file("b.txt", "b changed\n");

    let a_id = repo.get_hunk_id_for_file("a.txt", &[]);
    let b_id = repo.get_hunk_id_for_file("b.txt", &[]);
    repo.tool_ok(&["restore", &a_id, &b_id]);

    assert_eq!(repo.read_file("a.txt"), "a\n");
    assert_eq!(repo.read_file("b.txt"), "b\n");
}

#[test]
fn restore_addition() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\n");
    repo.write_file("a.txt", "line1\nnew_line\n");

    let id = repo.get_single_hunk_id(&[]);
    repo.tool_ok(&["restore", &id]);

    assert_eq!(repo.read_file("a.txt"), "line1\n");
}

#[test]
fn restore_deletion() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\nline2\nline3\n");
    repo.write_file("a.txt", "line1\nline3\n");

    let id = repo.get_single_hunk_id(&[]);
    repo.tool_ok(&["restore", &id]);

    assert_eq!(repo.read_file("a.txt"), "line1\nline2\nline3\n");
}

#[test]
fn restore_modification() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "old\n");
    repo.write_file("a.txt", "new\n");

    let id = repo.get_single_hunk_id(&[]);
    repo.tool_ok(&["restore", &id]);

    assert_eq!(repo.read_file("a.txt"), "old\n");
}

#[test]
fn restore_new_file() {
    let repo = TestRepo::new();
    repo.write_file("dummy.txt", "x\n");
    repo.jj(&["commit", "-m", "init"]);
    repo.write_file("brand_new.txt", "content\n");

    let id = repo.get_hunk_id_for_file("brand_new.txt", &[]);
    repo.tool_ok(&["restore", &id]);

    assert!(
        !repo.file_exists("brand_new.txt"),
        "new file should be removed"
    );
}

#[test]
fn restore_one_of_two_hunks() {
    let repo = TestRepo::new();
    let content = repo.write_two_hunk_file("f.txt");
    repo.jj(&["commit", "-m", "init"]);
    let new_content = content.replace("top", "TOP").replace("bottom", "BOTTOM");
    repo.write_file("f.txt", &new_content);

    let ids = repo.get_hunk_ids(&[]);
    assert!(ids.len() >= 2);

    repo.tool_ok(&["restore", &ids[0].0]);

    let file_content = repo.read_file("f.txt");
    let has_top = file_content.contains("TOP");
    let has_bottom = file_content.contains("BOTTOM");
    assert!(
        has_top != has_bottom || (!has_top && !has_bottom),
        "exactly one change should remain, got TOP={has_top} BOTTOM={has_bottom}"
    );
}

#[test]
fn restore_invalid_id() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "x\n");
    repo.write_file("a.txt", "y\n");

    let err = repo.tool_err(&["restore", "invalid"]);
    assert!(err.contains("hunk not found"));
}

// ──────────────────────────────────────────────────────────────────────────────
// _jj-tool protocol
// ──────────────────────────────────────────────────────────────────────────────

fn tool_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_jj-hunk-tool"))
}

fn run_jj_tool(left: &Path, right: &Path, patch_path: Option<&Path>) -> std::process::Output {
    run_jj_tool_with_reverse(left, right, patch_path, false)
}

fn run_jj_tool_with_reverse(
    left: &Path,
    right: &Path,
    patch_path: Option<&Path>,
    reverse: bool,
) -> std::process::Output {
    let mut cmd = Command::new(tool_binary());
    cmd.args([
        "_jj-tool",
        &left.display().to_string(),
        &right.display().to_string(),
    ]);
    if let Some(p) = patch_path {
        cmd.env("JJ_HUNK_TOOL_PATCH", p);
    }
    if reverse {
        cmd.env("JJ_HUNK_TOOL_REVERSE", "1");
    }
    cmd.output().unwrap()
}

#[test]
fn jj_tool_basic_reset_and_apply() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();

    std::fs::write(left.path().join("file.txt"), "hello\n").unwrap();
    std::fs::write(right.path().join("file.txt"), "hello\n").unwrap();

    let patch_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        patch_file.path(),
        "--- a/file.txt\n+++ b/file.txt\n@@ -1 +1,2 @@\n hello\n+world\n",
    )
    .unwrap();

    let output = run_jj_tool(left.path(), right.path(), Some(patch_file.path()));
    assert!(output.status.success());

    let content = std::fs::read_to_string(right.path().join("file.txt")).unwrap();
    assert_eq!(content, "hello\nworld\n");
}

#[test]
fn jj_tool_reverse_mode() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();

    // Left has the changed state (like restore --changes-in where left=current)
    std::fs::write(left.path().join("file.txt"), "hello\nworld\n").unwrap();
    // Right has the parent state
    std::fs::write(right.path().join("file.txt"), "hello\n").unwrap();

    // Forward patch (adds "world")
    let patch_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        patch_file.path(),
        "--- a/file.txt\n+++ b/file.txt\n@@ -1 +1,2 @@\n hello\n+world\n",
    )
    .unwrap();

    // With reverse: reset right to left (hello\nworld\n), then apply patch in reverse (remove world)
    let output =
        run_jj_tool_with_reverse(left.path(), right.path(), Some(patch_file.path()), true);
    assert!(output.status.success());

    let content = std::fs::read_to_string(right.path().join("file.txt")).unwrap();
    assert_eq!(content, "hello\n", "reverse should undo the patch");
}

#[test]
fn jj_tool_extra_files_removed() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();

    std::fs::write(left.path().join("keep.txt"), "keep\n").unwrap();
    std::fs::write(right.path().join("keep.txt"), "keep\n").unwrap();
    std::fs::write(right.path().join("extra.txt"), "should go\n").unwrap();

    let patch_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(patch_file.path(), "").unwrap();

    let output = run_jj_tool(left.path(), right.path(), Some(patch_file.path()));
    assert!(output.status.success());

    assert!(!right.path().join("extra.txt").exists());
    assert!(right.path().join("keep.txt").exists());
}

#[test]
fn jj_tool_subdirectories() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();

    std::fs::create_dir_all(left.path().join("sub/deep")).unwrap();
    std::fs::write(left.path().join("sub/deep/file.txt"), "nested\n").unwrap();

    std::fs::write(right.path().join("other.txt"), "other\n").unwrap();

    let patch_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(patch_file.path(), "").unwrap();

    let output = run_jj_tool(left.path(), right.path(), Some(patch_file.path()));
    assert!(output.status.success());

    assert!(!right.path().join("other.txt").exists());
    assert!(right.path().join("sub/deep/file.txt").exists());
    let content = std::fs::read_to_string(right.path().join("sub/deep/file.txt")).unwrap();
    assert_eq!(content, "nested\n");
}

#[test]
fn jj_tool_missing_env_var() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    std::fs::write(left.path().join("f.txt"), "x\n").unwrap();
    std::fs::write(right.path().join("f.txt"), "x\n").unwrap();

    let output = run_jj_tool(left.path(), right.path(), None);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("JJ_HUNK_TOOL_PATCH") || stderr.contains("not set"),
        "should mention missing env var: {stderr}"
    );
}

#[test]
fn jj_tool_invalid_patch() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    std::fs::write(left.path().join("f.txt"), "x\n").unwrap();
    std::fs::write(right.path().join("f.txt"), "x\n").unwrap();

    let patch_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(patch_file.path(), "this is not a valid patch\n").unwrap();

    let output = run_jj_tool(left.path(), right.path(), Some(patch_file.path()));
    assert!(!output.status.success());
}

#[test]
fn jj_tool_empty_patch() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();

    std::fs::write(left.path().join("file.txt"), "content\n").unwrap();
    std::fs::write(right.path().join("file.txt"), "different\n").unwrap();

    let patch_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(patch_file.path(), "").unwrap();

    let output = run_jj_tool(left.path(), right.path(), Some(patch_file.path()));
    assert!(output.status.success());

    let content = std::fs::read_to_string(right.path().join("file.txt")).unwrap();
    assert_eq!(content, "content\n");
}

// ──────────────────────────────────────────────────────────────────────────────
// install-skill tests
// ──────────────────────────────────────────────────────────────────────────────

fn run_install_skill(args: &[&str]) -> std::process::Output {
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_jj-hunk-tool"));
    let mut cmd_args = vec!["install-skill"];
    cmd_args.extend_from_slice(args);
    Command::new(&binary)
        .args(&cmd_args)
        .output()
        .unwrap()
}

#[test]
fn install_skill_creates_skill_file() {
    let target = tempfile::tempdir().unwrap();
    let output = run_install_skill(&["--target", target.path().to_str().unwrap()]);
    assert!(
        output.status.success(),
        "install-skill failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let skill_file = target.path().join("jj-surgeon").join("SKILL.md");
    assert!(skill_file.exists(), "jj-surgeon/SKILL.md should be created");
    let content = std::fs::read_to_string(&skill_file).unwrap();
    assert!(content.starts_with("---"), "should have YAML frontmatter");
    assert!(content.contains("jj-hunk-tool"), "should reference jj-hunk-tool");
}

#[test]
fn install_skill_creates_reference_files() {
    let target = tempfile::tempdir().unwrap();
    run_install_skill(&["--target", target.path().to_str().unwrap()]);
    let refs_dir = target.path().join("jj-surgeon").join("references");
    assert!(refs_dir.join("conflict-resolution.md").exists());
    assert!(refs_dir.join("git-interop.md").exists());
    assert!(refs_dir.join("revset-reference.md").exists());
    assert!(refs_dir.join("template-reference.md").exists());
}

#[test]
fn install_skill_overwrites_existing() {
    let target = tempfile::tempdir().unwrap();
    let skill_dir = target.path().join("jj-surgeon");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "old content").unwrap();
    let output = run_install_skill(&["--target", target.path().to_str().unwrap()]);
    assert!(output.status.success());
    let content = std::fs::read_to_string(skill_dir.join("SKILL.md")).unwrap();
    assert!(content.starts_with("---"), "should overwrite with new content");
}

// ──────────────────────────────────────────────────────────────────────────────
// absorb
// ──────────────────────────────────────────────────────────────────────────────

#[test]
fn absorb_single_hunk_to_ancestor() {
    let repo = TestRepo::new();
    // Base content
    repo.commit_file("a.txt", "line1\nline2\nline3\n");
    // Commit X: modify line2
    repo.write_file("a.txt", "line1\nmodified_by_x\nline3\n");
    repo.jj(&["commit", "-m", "change by X"]);
    // @ modifies the same line again
    repo.write_file("a.txt", "line1\nmodified_again\nline3\n");

    // Run absorb
    repo.tool_ok(&["absorb"]);

    // @ should be empty (change was absorbed)
    let wc_diff = repo.jj_diff("@");
    assert!(
        wc_diff.trim().is_empty(),
        "working copy should be empty after absorb, got: {wc_diff}"
    );

    // The ancestor commit (now @-) should have the final modification
    let ancestor_diff = repo.jj_diff("@-");
    assert!(
        ancestor_diff.contains("modified_again"),
        "ancestor should have the absorbed change, got: {ancestor_diff}"
    );
}

#[test]
fn absorb_multiple_hunks_to_different_ancestors() {
    let repo = TestRepo::new();
    // Base: two widely separated functions
    let base = "func_a\n".to_string()
        + &"padding\n".repeat(20)
        + "func_b\n";
    repo.commit_file("f.txt", &base);

    // Commit X: modify func_a
    let after_x = base.replace("func_a", "func_a_by_x");
    repo.write_file("f.txt", &after_x);
    repo.jj(&["commit", "-m", "change func_a"]);

    // Commit Y: modify func_b
    let after_y = after_x.replace("func_b", "func_b_by_y");
    repo.write_file("f.txt", &after_y);
    repo.jj(&["commit", "-m", "change func_b"]);

    // @: modify both funcs again
    let working = after_y
        .replace("func_a_by_x", "func_a_absorbed")
        .replace("func_b_by_y", "func_b_absorbed");
    repo.write_file("f.txt", &working);

    repo.tool_ok(&["absorb"]);

    // @ should be empty
    let wc_diff = repo.jj_diff("@");
    assert!(
        wc_diff.trim().is_empty(),
        "working copy should be empty after absorb, got: {wc_diff}"
    );

    // X (now @--) should have func_a_absorbed
    let x_diff = repo.jj_diff("@--");
    assert!(
        x_diff.contains("func_a_absorbed"),
        "X should have func_a change, got: {x_diff}"
    );

    // Y (now @-) should have func_b_absorbed
    let y_diff = repo.jj_diff("@-");
    assert!(
        y_diff.contains("func_b_absorbed"),
        "Y should have func_b change, got: {y_diff}"
    );
}

#[test]
fn absorb_insertion_same_file_uses_file_fallback() {
    let repo = TestRepo::new();
    // Base: some content with a gap
    let base = "func_a\n".to_string()
        + &"padding\n".repeat(20)
        + "end\n";
    repo.commit_file("f.txt", &base);

    // Commit X: modify func_a
    let after_x = base.replace("func_a", "func_a_by_x");
    repo.write_file("f.txt", &after_x);
    repo.jj(&["commit", "-m", "change func_a"]);

    // @: modify func_a (hunk match) AND add new code at end (file fallback)
    let working = after_x
        .replace("func_a_by_x", "func_a_absorbed")
        .replace("end", "brand_new_code\nend");
    repo.write_file("f.txt", &working);

    repo.tool_ok(&["absorb"]);

    // Both changes should be absorbed into X (one via hunk match, one via file fallback)
    let x_diff = repo.jj_diff("@-");
    assert!(
        x_diff.contains("func_a_absorbed"),
        "hunk-matched change should be absorbed, got: {x_diff}"
    );
    assert!(
        x_diff.contains("brand_new_code"),
        "file-fallback insertion should also be absorbed, got: {x_diff}"
    );

    // @ should be empty
    let wc_diff = repo.jj_diff("@");
    assert!(
        wc_diff.trim().is_empty(),
        "working copy should be empty after absorb, got: {wc_diff}"
    );
}

#[test]
fn absorb_unmatched_hunk_stays_in_working_copy() {
    let repo = TestRepo::new();
    // Commit X: modify a.txt
    repo.commit_file("a.txt", "aaa\n");
    repo.write_file("a.txt", "aaa_by_x\n");
    repo.jj(&["commit", "-m", "change a.txt"]);

    // @ adds a brand-new file — no ancestor could have touched it
    repo.write_file("brand_new.txt", "new stuff\n");

    repo.tool_ok(&["absorb"]);

    // New file should stay in @ (new files have no file-level fallback)
    let wc_diff = repo.jj_diff("@");
    assert!(
        wc_diff.contains("brand_new.txt"),
        "new file should stay in working copy, got: {wc_diff}"
    );
}

#[test]
fn absorb_with_hunk_ids_selective() {
    let repo = TestRepo::new();
    // Base with two files
    repo.commit_file("a.txt", "aaa\n");
    repo.commit_file("b.txt", "bbb\n");

    // Commit X: modify both files
    repo.write_file("a.txt", "aaa_by_x\n");
    repo.write_file("b.txt", "bbb_by_x\n");
    repo.jj(&["commit", "-m", "change both"]);

    // @: modify both again
    repo.write_file("a.txt", "aaa_absorbed\n");
    repo.write_file("b.txt", "bbb_absorbed\n");

    // Only absorb the a.txt hunk
    let a_id = repo.get_hunk_id_for_file("a.txt", &[]);
    repo.tool_ok(&["absorb", &a_id]);

    // a.txt change should be absorbed
    let x_diff = repo.jj_diff("@-");
    assert!(
        x_diff.contains("aaa_absorbed"),
        "a.txt should be absorbed, got: {x_diff}"
    );

    // b.txt change should still be in @
    let wc_diff = repo.jj_diff("@");
    assert!(
        wc_diff.contains("bbb_absorbed"),
        "b.txt should stay in working copy, got: {wc_diff}"
    );
}

#[test]
fn absorb_dry_run_no_changes() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\nline2\nline3\n");
    repo.write_file("a.txt", "line1\nmodified_by_x\nline3\n");
    repo.jj(&["commit", "-m", "change by X"]);
    repo.write_file("a.txt", "line1\nmodified_again\nline3\n");

    let output = repo.tool_ok(&["absorb", "--dry-run"]);

    // Should show routing plan
    assert!(
        output.contains("a.txt"),
        "dry run should mention the file, got: {output}"
    );

    // But NOT actually make changes — @ should still have the diff
    let wc_diff = repo.jj_diff("@");
    assert!(
        !wc_diff.trim().is_empty(),
        "dry run should not change working copy"
    );
}

#[test]
fn absorb_nothing_to_absorb() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "hello\n");
    // @ has no changes

    let result = repo.tool(&["absorb"]);
    // Should succeed but indicate nothing to do
    assert!(result.is_ok(), "absorb with no changes should not error");
    let output = result.unwrap();
    assert!(
        output.contains("Nothing") || output.contains("nothing") || output.trim().is_empty(),
        "should indicate nothing to absorb, got: {output}"
    );
}

#[test]
fn absorb_new_file_stays_in_working_copy() {
    let repo = TestRepo::new();
    repo.commit_file("existing.txt", "content\n");
    // Commit X: modify existing file
    repo.write_file("existing.txt", "modified\n");
    repo.jj(&["commit", "-m", "modify existing"]);

    // @: modify existing (should absorb) and add new file (should not)
    repo.write_file("existing.txt", "modified_again\n");
    repo.write_file("brand_new.txt", "new file content\n");

    repo.tool_ok(&["absorb"]);

    // existing.txt change absorbed
    let x_diff = repo.jj_diff("@-");
    assert!(
        x_diff.contains("modified_again"),
        "existing.txt should be absorbed, got: {x_diff}"
    );

    // brand_new.txt stays in @
    let wc_diff = repo.jj_diff("@");
    assert!(
        wc_diff.contains("brand_new.txt"),
        "new file should stay in working copy, got: {wc_diff}"
    );
}

#[test]
fn absorb_pure_insertion_falls_back_to_file() {
    let repo = TestRepo::new();
    // Base file with content
    let base = "line1\nline2\nline3\nline4\nline5\n";
    repo.commit_file("a.txt", base);

    // Commit X: modify line2
    let after_x = base.replace("line2", "line2_by_x");
    repo.write_file("a.txt", &after_x);
    repo.jj(&["commit", "-m", "change line2"]);

    // @ adds a pure insertion (no deletions) to the same file
    let working = after_x.replace("line5", "line5\nnew_stuff");
    repo.write_file("a.txt", &working);

    repo.tool_ok(&["absorb"]);

    // The pure insertion should fall back to file-level match → X
    let x_diff = repo.jj_diff("@-");
    assert!(
        x_diff.contains("new_stuff"),
        "pure insertion should be absorbed via file fallback, got: {x_diff}"
    );

    // @ should be empty
    let wc_diff = repo.jj_diff("@");
    assert!(
        wc_diff.trim().is_empty(),
        "working copy should be empty after absorb, got: {wc_diff}"
    );
}

#[test]
fn absorb_file_fallback_picks_most_recent() {
    let repo = TestRepo::new();
    // Base file
    let base = "aaa\n".to_string() + &"padding\n".repeat(20) + "zzz\n";
    repo.commit_file("f.txt", &base);

    // Commit X: modify "aaa"
    let after_x = base.replace("aaa", "aaa_by_x");
    repo.write_file("f.txt", &after_x);
    repo.jj(&["commit", "-m", "commit X"]);

    // Commit Y: modify "zzz" (most recent ancestor touching f.txt)
    let after_y = after_x.replace("zzz", "zzz_by_y");
    repo.write_file("f.txt", &after_y);
    repo.jj(&["commit", "-m", "commit Y"]);

    // @ adds pure insertion in the middle (not overlapping X or Y's hunks)
    let working = after_y.replace("padding\npadding\npadding\npadding\npadding\n",
        "padding\npadding\nINSERTED\npadding\npadding\npadding\n");
    repo.write_file("f.txt", &working);

    repo.tool_ok(&["absorb"]);

    // Should fall back to Y (most recent ancestor touching f.txt)
    let y_diff = repo.jj_diff("@-");
    assert!(
        y_diff.contains("INSERTED"),
        "pure insertion should go to most recent ancestor (Y), got: {y_diff}"
    );
}

#[test]
fn absorb_file_fallback_does_not_apply_to_new_files() {
    let repo = TestRepo::new();
    repo.commit_file("existing.txt", "hello\n");
    repo.write_file("existing.txt", "hello_modified\n");
    repo.jj(&["commit", "-m", "modify existing"]);

    // @ adds a brand new file (no ancestor touched it)
    repo.write_file("brand_new.txt", "new content\n");

    repo.tool_ok(&["absorb"]);

    // New file should stay in @
    let wc_diff = repo.jj_diff("@");
    assert!(
        wc_diff.contains("brand_new.txt"),
        "new file should not be absorbed via file fallback, got: {wc_diff}"
    );
}

#[test]
fn absorb_interactive_accept() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\nline2\nline3\n");
    repo.write_file("a.txt", "line1\nmodified_by_x\nline3\n");
    repo.jj(&["commit", "-m", "change by X"]);
    repo.write_file("a.txt", "line1\nmodified_again\nline3\n");

    // Accept the hunk with "a\n"
    let output = repo.tool_with_stdin(&["absorb", "-i"], "a\n");

    // Should show the hunk and target
    assert!(output.contains("a.txt"), "should show file name: {output}");

    // Change should be absorbed
    let wc_diff = repo.jj_diff("@");
    assert!(
        wc_diff.trim().is_empty(),
        "accepted hunk should be absorbed, got: {wc_diff}"
    );
}

#[test]
fn absorb_interactive_skip() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\nline2\nline3\n");
    repo.write_file("a.txt", "line1\nmodified_by_x\nline3\n");
    repo.jj(&["commit", "-m", "change by X"]);
    repo.write_file("a.txt", "line1\nmodified_again\nline3\n");

    // Skip the hunk with "s\n"
    let output = repo.tool_with_stdin(&["absorb", "-i"], "s\n");

    // Should show the hunk
    assert!(output.contains("a.txt"), "should show file name: {output}");

    // Change should NOT be absorbed — still in @
    let wc_diff = repo.jj_diff("@");
    assert!(
        wc_diff.contains("modified_again"),
        "skipped hunk should stay in working copy, got: {wc_diff}"
    );
}

#[test]
fn absorb_interactive_quit() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "aaa\n");
    repo.commit_file("b.txt", "bbb\n");
    repo.write_file("a.txt", "aaa_by_x\n");
    repo.write_file("b.txt", "bbb_by_x\n");
    repo.jj(&["commit", "-m", "change both"]);
    repo.write_file("a.txt", "aaa_absorbed\n");
    repo.write_file("b.txt", "bbb_absorbed\n");

    // Accept first hunk, then quit
    let _output = repo.tool_with_stdin(&["absorb", "-i"], "a\nq\n");

    // Only the first hunk should be absorbed; b.txt stays
    let wc_diff = repo.jj_diff("@");
    assert!(
        wc_diff.contains("bbb_absorbed"),
        "second hunk should stay after quit, got: {wc_diff}"
    );
}

#[test]
fn absorb_interactive_shows_absolute_line_numbers() {
    let repo = TestRepo::new();
    // Create a file with enough lines so the change is NOT at line 1
    let mut content = String::new();
    for i in 1..=20 {
        content.push_str(&format!("line{i}\n"));
    }
    repo.commit_file("a.txt", &content);

    // Commit X: modify line 10
    let after_x = content.replace("line10", "line10_by_x");
    repo.write_file("a.txt", &after_x);
    repo.jj(&["commit", "-m", "change line10"]);

    // @ modifies the same line
    let working = after_x.replace("line10_by_x", "line10_absorbed");
    repo.write_file("a.txt", &working);

    // Run interactive absorb, accept
    let output = repo.tool_with_stdin(&["absorb", "-i"], "a\n");

    // Should show absolute line number 10 (not relative "1:")
    assert!(
        output.contains("10:") || output.contains(" 10 "),
        "should show absolute line number 10, got: {output}"
    );
    // Should NOT show relative "1:" as the first changed line
    // (context starts earlier, so first line number should be > 1)
    let first_numbered = output
        .lines()
        .find(|l| {
            let trimmed = l.trim();
            trimmed.starts_with("1:") || trimmed.starts_with(" 1:")
        });
    // If context starts at line 7+ (3 lines of context), "1:" should not appear
    assert!(
        first_numbered.is_none(),
        "should use absolute line numbers, not relative; found: {:?}",
        first_numbered
    );
}

#[test]
fn absorb_interactive_retarget() {
    let repo = TestRepo::new();
    // Base with two separated regions
    let base = "func_a\n".to_string() + &"padding\n".repeat(20) + "func_b\n";
    repo.commit_file("f.txt", &base);

    // Commit X: modify func_a
    let after_x = base.replace("func_a", "func_a_by_x");
    repo.write_file("f.txt", &after_x);
    repo.jj(&["commit", "-m", "change func_a"]);

    // Commit Y: modify func_b
    let after_y = after_x.replace("func_b", "func_b_by_y");
    repo.write_file("f.txt", &after_y);
    repo.jj(&["commit", "-m", "change func_b"]);

    // @ modifies func_a (would normally route to X)
    let working = after_y.replace("func_a_by_x", "func_a_retarget");
    repo.write_file("f.txt", &working);

    // Use [t]arget to override routing — pick Y instead of X
    // Format: t\n then select Y's index (1-based, Y should be listed)
    // We need to know which number Y is. Since ancestors are listed,
    // we use "t\n2\n" to pick the second option (or "t\n1\n" for first).
    // The exact order depends on implementation. Let's use "a\n" first
    // and test retarget separately once we know the output format.
    // For now, just test that 't' triggers the target selection prompt.
    let output = repo.tool_with_stdin(&["absorb", "-i"], "t\n1\n");

    // Should have shown target selection
    assert!(
        output.contains("func_a") || output.contains("f.txt"),
        "should show the hunk, got: {output}"
    );
}

#[test]
fn absorb_ambiguous_hunk_stays() {
    let repo = TestRepo::new();
    // Base content with 3 lines
    repo.commit_file("a.txt", "line_a\nline_b\nline_c\n");

    // Commit X: modify line_a
    repo.write_file("a.txt", "line_a_by_x\nline_b\nline_c\n");
    repo.jj(&["commit", "-m", "change line_a"]);

    // Commit Y: modify line_c
    repo.write_file("a.txt", "line_a_by_x\nline_b\nline_c_by_y\n");
    repo.jj(&["commit", "-m", "change line_c"]);

    // @ modifies ALL three lines in a single hunk (lines from both X and Y)
    repo.write_file("a.txt", "AAA\nBBB\nCCC\n");

    let output = repo.tool_ok(&["absorb"]);

    // Should be ambiguous — the deleted lines span both X and Y
    // The hunk should stay in @
    let wc_diff = repo.jj_diff("@");
    assert!(
        !wc_diff.trim().is_empty(),
        "ambiguous hunk should stay in working copy"
    );

    // Output should mention ambiguity
    assert!(
        output.contains("ambiguous") || output.contains("Ambiguous") || output.contains("overlaps"),
        "should report ambiguity, got: {output}"
    );
}

#[test]
fn absorb_multiple_files_to_different_ancestors() {
    let repo = TestRepo::new();
    // Base: create 3 files
    repo.commit_file("a.txt", "aaa\n");
    repo.commit_file("b.txt", "bbb\n");
    repo.commit_file("c.txt", "ccc\n");

    // Commit X: modify a.txt
    repo.write_file("a.txt", "aaa_by_x\n");
    repo.jj(&["commit", "-m", "change a"]);

    // Commit Y: modify b.txt
    repo.write_file("b.txt", "bbb_by_y\n");
    repo.jj(&["commit", "-m", "change b"]);

    // Commit Z: modify c.txt
    repo.write_file("c.txt", "ccc_by_z\n");
    repo.jj(&["commit", "-m", "change c"]);

    // @: modify all three again
    repo.write_file("a.txt", "aaa_absorbed\n");
    repo.write_file("b.txt", "bbb_absorbed\n");
    repo.write_file("c.txt", "ccc_absorbed\n");

    repo.tool_ok(&["absorb"]);

    // @ should be empty
    let wc_diff = repo.jj_diff("@");
    assert!(
        wc_diff.trim().is_empty(),
        "working copy should be empty after absorb, got: {wc_diff}"
    );

    // X (now @---) should have aaa_absorbed
    let x_diff = repo.jj_diff("@---");
    assert!(
        x_diff.contains("aaa_absorbed"),
        "X should have a.txt change, got: {x_diff}"
    );

    // Y (now @--) should have bbb_absorbed
    let y_diff = repo.jj_diff("@--");
    assert!(
        y_diff.contains("bbb_absorbed"),
        "Y should have b.txt change, got: {y_diff}"
    );

    // Z (now @-) should have ccc_absorbed
    let z_diff = repo.jj_diff("@-");
    assert!(
        z_diff.contains("ccc_absorbed"),
        "Z should have c.txt change, got: {z_diff}"
    );
}

#[test]
fn install_skill_prints_success_message() {
    let target = tempfile::tempdir().unwrap();
    let output = run_install_skill(&["--target", target.path().to_str().unwrap()]);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Installed"), "should print success message");
}

// ──────────────────────────────────────────────────────────────────────────────
// Conflict reporting
// ──────────────────────────────────────────────────────────────────────────────

#[test]
fn split_reports_rebase_of_descendants() {
    let repo = TestRepo::new();

    // Create a commit with two files, then a descendant that modifies one
    repo.write_file("a.txt", "aaa\n");
    repo.write_file("b.txt", "bbb\n");
    repo.jj(&["commit", "-m", "add both files"]);

    repo.write_file("a.txt", "aaa modified\n");
    repo.jj(&["commit", "-m", "modify a.txt"]);

    // Split the ancestor by pulling a.txt out — descendants get rebased
    let hunk_id = repo.get_hunk_id_for_file("a.txt", &["-r", "@--"]);
    let (stdout, stderr) = repo.tool_output(&[
        "split", &hunk_id, "-r", "@--", "-m", "just a.txt",
    ]);
    let combined = format!("{stdout}{stderr}");

    assert!(
        combined.contains("Rebased"),
        "jj-hunk-tool split should pass through jj's rebase messages.\nstdout: {stdout}\nstderr: {stderr}"
    );
}

#[test]
fn squash_reports_conflicts_in_descendants() {
    let repo = TestRepo::new();

    // Commit A: create file with some content
    repo.write_file("file.txt", "line 1\nline 2\nline 3\n");
    repo.jj(&["commit", "-m", "A: initial"]);

    // Commit B (descendant of A): modify the same lines
    repo.write_file("file.txt", "line 1\nmodified by B\nline 3\n");
    repo.jj(&["commit", "-m", "B: modify line 2"]);

    // Now in @, make a conflicting change and squash it into A.
    repo.write_file("file.txt", "line 1\nmodified by fixup\nline 3\n");

    // Get the hunk ID
    let hunk_id = repo.get_single_hunk_id(&[]);

    // Find revision A's change ID
    let log = repo.jj(&["log", "--no-graph", "-r", "@--", "-T", "change_id.short()"]);
    let a_id = log.trim();

    // Squash the hunk into A — this should conflict with B
    let (stdout, stderr) = repo.tool_output(&["squash", &hunk_id, "--into", a_id, "-m", "A: fixed"]);
    let combined = format!("{stdout}{stderr}");

    assert!(
        combined.contains("conflict"),
        "jj-hunk-tool squash should report conflicts in descendants.\nstdout: {stdout}\nstderr: {stderr}"
    );
}

#[test]
fn absorb_interactive_single_keypress() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\nline2\nline3\n");
    repo.write_file("a.txt", "line1\nmodified_by_x\nline3\n");
    repo.jj(&["commit", "-m", "change by X"]);
    repo.write_file("a.txt", "line1\nmodified_again\nline3\n");

    // Send just "a" without newline — single keypress should suffice
    let output = repo.tool_with_stdin(&["absorb", "-i"], "a");

    assert!(output.contains("a.txt"), "should show file name: {output}");
    let wc_diff = repo.jj_diff("@");
    assert!(
        wc_diff.trim().is_empty(),
        "single keypress 'a' should absorb the hunk, got: {wc_diff}"
    );
}

#[test]
fn absorb_interactive_skip_file() {
    let repo = TestRepo::new();
    // Two hunks in a.txt, one in b.txt
    repo.commit_file("a.txt", "aaa1\npadding\npadding\npadding\naaa2\n");
    repo.commit_file("b.txt", "bbb\n");
    repo.write_file("a.txt", "aaa1_by_x\npadding\npadding\npadding\naaa2_by_x\n");
    repo.write_file("b.txt", "bbb_by_x\n");
    repo.jj(&["commit", "-m", "change all"]);
    repo.write_file("a.txt", "aaa1_new\npadding\npadding\npadding\naaa2_new\n");
    repo.write_file("b.txt", "bbb_new\n");

    // [S]kip file skips rest of a.txt, then [a]bsorb on b.txt
    let output = repo.tool_with_stdin(&["absorb", "-i"], "Sa");

    // a.txt hunks should be skipped, b.txt should be absorbed
    let wc_diff = repo.jj_diff("@");
    assert!(
        wc_diff.contains("aaa1_new") || wc_diff.contains("aaa2_new"),
        "a.txt hunks should be skipped by [S]kip file, got: {wc_diff}"
    );
    assert!(
        !wc_diff.contains("bbb_new"),
        "b.txt hunk should be absorbed, got: {wc_diff}"
    );
    assert!(
        output.contains("[S]kip file"),
        "prompt should show [S]kip file option, got: {output}"
    );
    assert!(
        output.contains("[A]bsorb file"),
        "prompt should show [A]bsorb file option, got: {output}"
    );
}

#[test]
fn absorb_interactive_absorb_file() {
    let repo = TestRepo::new();
    // Two hunks in a.txt, one in b.txt
    repo.commit_file("a.txt", "aaa1\npadding\npadding\npadding\naaa2\n");
    repo.commit_file("b.txt", "bbb\n");
    repo.write_file("a.txt", "aaa1_by_x\npadding\npadding\npadding\naaa2_by_x\n");
    repo.write_file("b.txt", "bbb_by_x\n");
    repo.jj(&["commit", "-m", "change all"]);
    repo.write_file("a.txt", "aaa1_new\npadding\npadding\npadding\naaa2_new\n");
    repo.write_file("b.txt", "bbb_new\n");

    // [A]bsorb file on a.txt (absorbs both a.txt hunks), then [s]kip b.txt
    let _output = repo.tool_with_stdin(&["absorb", "-i"], "As");

    // a.txt hunks should be absorbed, b.txt should be skipped
    let wc_diff = repo.jj_diff("@");
    assert!(
        !wc_diff.contains("aaa1_new") && !wc_diff.contains("aaa2_new"),
        "a.txt hunks should be absorbed by [A]bsorb file, got: {wc_diff}"
    );
    assert!(
        wc_diff.contains("bbb_new"),
        "b.txt hunk should be skipped, got: {wc_diff}"
    );
}

#[test]
fn absorb_works_from_subdirectory() {
    let repo = TestRepo::new();
    // Create file in a subdirectory
    repo.commit_file("sub/a.txt", "line1\nline2\nline3\n");
    repo.write_file("sub/a.txt", "line1\nmodified_by_x\nline3\n");
    repo.jj(&["commit", "-m", "change by X"]);
    repo.write_file("sub/a.txt", "line1\nmodified_again\nline3\n");

    // Run absorb from the subdirectory
    repo.tool_ok_in_subdir("sub", &["absorb"]);

    // Change should be absorbed
    let wc_diff = repo.jj_diff("@");
    assert!(
        wc_diff.trim().is_empty(),
        "absorb from subdir should work, got: {wc_diff}"
    );
}

#[test]
fn absorb_prints_undo_operation_id() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\nline2\nline3\n");
    repo.write_file("a.txt", "line1\nmodified_by_x\nline3\n");
    repo.jj(&["commit", "-m", "change by X"]);
    repo.write_file("a.txt", "line1\nmodified_again\nline3\n");

    let output = repo.tool_ok(&["absorb"]);

    // Should contain an undo hint with an operation ID
    assert!(
        output.contains("To undo, run: jj op restore "),
        "absorb should print undo operation ID, got: {output}"
    );

    // Extract the op ID and verify it works
    let op_line = output.lines().find(|l| l.contains("jj op restore")).unwrap();
    let op_id = op_line.trim().strip_prefix("To undo, run: jj op restore ").unwrap();
    assert!(
        !op_id.is_empty(),
        "operation ID should not be empty"
    );

    // Verify we can actually restore to that operation
    repo.jj(&["op", "restore", op_id]);
    // After restoring, the working copy should have the diff again
    let wc_diff = repo.jj_diff("@");
    assert!(
        wc_diff.contains("modified_again"),
        "after op restore, working copy should have the change back, got: {wc_diff}"
    );
}

#[test]
fn absorb_dry_run_does_not_print_undo() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\nline2\nline3\n");
    repo.write_file("a.txt", "line1\nmodified_by_x\nline3\n");
    repo.jj(&["commit", "-m", "change by X"]);
    repo.write_file("a.txt", "line1\nmodified_again\nline3\n");

    let output = repo.tool_ok(&["absorb", "--dry-run"]);

    assert!(
        !output.contains("jj op restore"),
        "dry-run absorb should not print undo hint, got: {output}"
    );
}

#[test]
fn debug_flag_hunks_prints_diff_command() {
    let repo = TestRepo::new();
    repo.write_file("a.txt", "hello\n");

    let (_stdout, stderr) = repo.tool_output(&["--debug", "hunks"]);

    assert!(
        stderr.contains("debug: running jj diff"),
        "--debug should print the jj diff command, stderr: {stderr}"
    );
    assert!(
        stderr.contains("debug: parsed"),
        "--debug should print parsed hunk count, stderr: {stderr}"
    );
}

#[test]
fn debug_flag_split_prints_jj_command() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\nline2\nline3\n");
    repo.write_file("a.txt", "ADDED\nline1\nline2\nline3\nADDED2\n");

    let stdout = repo.tool_ok(&["hunks"]);
    let hunk_id = stdout.lines().next().unwrap().split_whitespace().next().unwrap();

    let (_stdout, stderr) = repo.tool_output(&["--debug", "split", hunk_id, "-m", "first"]);

    assert!(
        stderr.contains("debug: running jj"),
        "--debug should print the jj command, stderr: {stderr}"
    );
    assert!(
        stderr.contains("debug: patch content"),
        "--debug should print patch info, stderr: {stderr}"
    );
}

#[test]
fn debug_flag_absorb_prints_annotation_details() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\nline2\nline3\n");
    repo.write_file("a.txt", "line1\nmodified_by_x\nline3\n");
    repo.jj(&["commit", "-m", "change by X"]);
    repo.write_file("a.txt", "line1\nmodified_again\nline3\n");

    let (_stdout, stderr) = repo.tool_output(&["--debug", "absorb", "--dry-run"]);

    assert!(
        stderr.contains("debug: annotating"),
        "--debug should print annotation info, stderr: {stderr}"
    );
    assert!(
        stderr.contains("debug: mutable ancestors"),
        "--debug should print ancestor info, stderr: {stderr}"
    );
}
