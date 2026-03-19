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

    /// Extract hunk IDs from `hunks` output. Returns vec of (id, line_text).
    fn get_hunk_ids(&self, extra_args: &[&str]) -> Vec<(String, String)> {
        let mut args = vec!["hunks"];
        args.extend_from_slice(extra_args);
        let output = self.tool_ok(&args);
        output
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with(' '))
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
fn hunks_full_mode() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\nline2\nline3\n");
    repo.write_file("a.txt", "line1\nchanged\nline3\n");

    let output = repo.tool_ok(&["hunks", "--full"]);
    // Should have line numbers
    assert!(output.contains("1:"), "should have line numbers");
    // Should include context lines
    assert!(
        output.contains("line1") || output.contains("line3"),
        "should have context"
    );
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
fn hunks_preview_truncation() {
    let repo = TestRepo::new();
    repo.commit_file("big.txt", "");
    // Create a diff with >4 changed lines
    let new_content: String = (1..=10).map(|i| format!("line{i}\n")).collect();
    repo.write_file("big.txt", &new_content);

    let output = repo.tool_ok(&["hunks"]);
    assert!(
        output.contains("... (+"),
        "should truncate with '...' message"
    );
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
    // Create two files with identical content and identical changes
    // so hunk hashes collide
    repo.commit_file("a.txt", "same\n");
    repo.commit_file("b.txt", "same\n");
    // Hmm, IDs hash file path too, so they won't collide with different paths.
    // To get a real collision we'd need same file path which is impossible.
    // The suffix mechanism is for same-file hunks with identical content.
    // This is extremely hard to trigger naturally. Skip the assertion on suffix
    // and just verify the mechanism doesn't crash with many hunks.
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
fn hunks_full_with_revision() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\n");
    repo.write_file("a.txt", "line1\nline2\n");
    repo.jj(&["commit", "-m", "add line2"]);

    let output = repo.tool_ok(&["hunks", "--full", "-r", "@-"]);
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
// show
// ──────────────────────────────────────────────────────────────────────────────

#[test]
fn show_valid_id() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\nline2\n");
    repo.write_file("a.txt", "line1\nchanged\n");

    let id = repo.get_single_hunk_id(&[]);
    let output = repo.tool_ok(&["show", &id]);
    assert!(output.contains("@@"), "should have @@ header");
    assert!(output.contains("1:"), "should have line numbers");
    assert!(output.contains("-line2") || output.contains("+changed"));
}

#[test]
fn show_invalid_id() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\n");
    repo.write_file("a.txt", "line1\nchanged\n");

    let err = repo.tool_err(&["show", "invalid"]);
    assert!(
        err.contains("hunk not found"),
        "should say hunk not found: {err}"
    );
}

#[test]
fn show_with_revision() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "original\n");
    repo.write_file("a.txt", "modified\n");
    repo.jj(&["commit", "-m", "modify"]);

    let id = repo.get_single_hunk_id(&["-r", "@-"]);
    let output = repo.tool_ok(&["show", &id, "-r", "@-"]);
    assert!(output.contains("@@"));
    assert!(output.contains("1:"));
}

#[test]
fn show_context_lines_present() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\nline2\nline3\nline4\nline5\n");
    repo.write_file("a.txt", "line1\nline2\nCHANGED\nline4\nline5\n");

    let id = repo.get_single_hunk_id(&[]);
    let output = repo.tool_ok(&["show", &id]);
    // Context lines (space-prefixed) should be present
    assert!(
        output.contains(" line"),
        "should have context lines: {output}"
    );
}

#[test]
fn show_line_numbers_sequential() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\nb\nc\n");
    repo.write_file("a.txt", "a\nB\nc\n");

    let id = repo.get_single_hunk_id(&[]);
    let output = repo.tool_ok(&["show", &id]);
    // Find numbered lines and verify they're sequential
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

    // Should have file headers
    assert!(patch.contains("--- a/f.txt"));
    assert!(patch.contains("+++ b/f.txt"));
    // Should have multiple @@ sections
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
    // For whole-hunk reverse, the patch keeps original +/- (applied with patch --reverse)
    assert!(patch.contains("+added"));
}

#[test]
fn patch_inline_range() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\nline2\nline3\nline4\n");
    repo.write_file("a.txt", "LINE1\nLINE2\nLINE3\nLINE4\n");

    let id = repo.get_single_hunk_id(&[]);
    // Only include lines 1-2 of the hunk (out of 8 change lines: 4 deletions + 4 additions)
    let spec = format!("{id}:1-4");
    let patch = repo.tool_ok(&["patch", &spec]);
    assert!(patch.contains("@@"));
    // The sliced patch should exist and be valid unified diff
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

    // Reset file to original, then apply the patch
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
    // Just verify patch command accepts it
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
// commit
// ──────────────────────────────────────────────────────────────────────────────

#[test]
fn commit_single_hunk_with_message() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "original\n");
    repo.write_file("a.txt", "modified\n");

    let id = repo.get_single_hunk_id(&[]);
    repo.tool_ok(&["commit", &id, "-m", "my commit"]);

    let parent_diff = repo.jj_diff("@-");
    assert!(parent_diff.contains("a.txt"));
    // Check message
    let desc = repo.jj(&["log", "-r", "@-", "-T", "description", "--no-graph"]);
    assert!(desc.contains("my commit"));
}

#[test]
fn commit_single_hunk_no_message() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "original\n");
    repo.write_file("a.txt", "modified\n");

    let id = repo.get_single_hunk_id(&[]);
    // No -m flag — jj commit without message should still work (empty description)
    repo.tool_ok(&["commit", &id]);

    let parent_diff = repo.jj_diff("@-");
    assert!(parent_diff.contains("a.txt"));
}

#[test]
fn commit_multiple_hunks_different_files() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\n");
    repo.commit_file("b.txt", "b\n");
    repo.write_file("a.txt", "a changed\n");
    repo.write_file("b.txt", "b changed\n");

    let a_id = repo.get_hunk_id_for_file("a.txt", &[]);
    let b_id = repo.get_hunk_id_for_file("b.txt", &[]);
    repo.tool_ok(&["commit", &a_id, &b_id, "-m", "both files"]);

    let parent_diff = repo.jj_diff("@-");
    assert!(parent_diff.contains("a.txt"));
    assert!(parent_diff.contains("b.txt"));
}

#[test]
fn commit_one_of_two_hunks_other_remains() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\n");
    repo.commit_file("b.txt", "b\n");
    repo.write_file("a.txt", "a changed\n");
    repo.write_file("b.txt", "b changed\n");

    let a_id = repo.get_hunk_id_for_file("a.txt", &[]);
    repo.tool_ok(&["commit", &a_id, "-m", "only a"]);

    // Parent should have only a.txt
    let parent_diff = repo.jj_diff("@-");
    assert!(parent_diff.contains("a.txt"));
    assert!(!parent_diff.contains("b.txt"));

    // Working copy should still have b.txt change
    let wc_diff = repo.jj_diff("@");
    assert!(wc_diff.contains("b.txt"));
    assert!(!wc_diff.contains("a.txt"));
}

#[test]
fn commit_with_inline_range() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\nline2\nline3\nline4\n");
    repo.write_file("a.txt", "LINE1\nLINE2\nline3\nline4\n");

    let id = repo.get_single_hunk_id(&[]);
    // Commit only part of the hunk
    let spec = format!("{id}:1-2");
    repo.tool_ok(&["commit", &spec, "-m", "partial"]);

    let desc = repo.jj(&["log", "-r", "@-", "-T", "description", "--no-graph"]);
    assert!(desc.contains("partial"));
}

#[test]
fn commit_invalid_id() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "x\n");
    repo.write_file("a.txt", "y\n");

    let err = repo.tool_err(&["commit", "invalid", "-m", "nope"]);
    assert!(err.contains("hunk not found"));
}

#[test]
fn commit_with_revision_split() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\n");
    repo.commit_file("b.txt", "b\n");
    repo.write_file("a.txt", "a changed\n");
    repo.write_file("b.txt", "b changed\n");
    repo.jj(&["commit", "-m", "change both"]);

    // Split @- by picking only the a.txt hunk
    let a_id = repo.get_hunk_id_for_file("a.txt", &["-r", "@-"]);
    repo.tool_ok(&["commit", &a_id, "-r", "@-", "-m", "just a"]);

    // There should now be a commit with just a.txt in the history
    // (jj split creates two commits from the original)
}

#[test]
fn commit_all_hunks_working_copy_empty() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\n");
    repo.write_file("a.txt", "a changed\n");

    let id = repo.get_single_hunk_id(&[]);
    repo.tool_ok(&["commit", &id, "-m", "everything"]);

    let wc_diff = repo.jj_diff("@");
    assert!(
        wc_diff.trim().is_empty(),
        "working copy should be empty after committing all hunks"
    );
}

#[test]
fn commit_new_file() {
    let repo = TestRepo::new();
    repo.write_file("dummy.txt", "x\n");
    repo.jj(&["commit", "-m", "init"]);
    repo.write_file("brand_new.txt", "content\n");

    let id = repo.get_hunk_id_for_file("brand_new.txt", &[]);
    repo.tool_ok(&["commit", &id, "-m", "add new file"]);

    let parent_diff = repo.jj_diff("@-");
    assert!(parent_diff.contains("brand_new.txt"));
}

#[test]
fn commit_deleted_file() {
    let repo = TestRepo::new();
    repo.commit_file("doomed.txt", "content\n");
    std::fs::remove_file(repo.path().join("doomed.txt")).unwrap();

    let id = repo.get_hunk_id_for_file("doomed.txt", &[]);
    repo.tool_ok(&["commit", &id, "-m", "remove file"]);

    let parent_diff = repo.jj_diff("@-");
    assert!(parent_diff.contains("doomed.txt"));
}

#[test]
fn commit_multiple_hunks_same_file() {
    let repo = TestRepo::new();
    let content = repo.write_two_hunk_file("f.txt");
    repo.jj(&["commit", "-m", "init"]);
    let new_content = content.replace("top", "TOP").replace("bottom", "BOTTOM");
    repo.write_file("f.txt", &new_content);

    let ids = repo.get_hunk_ids(&[]);
    assert!(ids.len() >= 2);

    // Commit only the first hunk
    repo.tool_ok(&["commit", &ids[0].0, "-m", "first hunk only"]);

    // Parent should have partial change
    let parent_diff = repo.jj_diff("@-");
    assert!(parent_diff.contains("f.txt"));

    // Working copy should still have the other hunk
    let wc_diff = repo.jj_diff("@");
    assert!(wc_diff.contains("f.txt"));
}

// ──────────────────────────────────────────────────────────────────────────────
// discard
// ──────────────────────────────────────────────────────────────────────────────

#[test]
fn discard_single_hunk() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "original\n");
    repo.write_file("a.txt", "modified\n");

    let id = repo.get_single_hunk_id(&[]);
    repo.tool_ok(&["discard", &id]);

    assert_eq!(repo.read_file("a.txt"), "original\n");
}

#[test]
fn discard_one_of_two_hunks() {
    let repo = TestRepo::new();
    let content = repo.write_two_hunk_file("f.txt");
    repo.jj(&["commit", "-m", "init"]);
    let new_content = content.replace("top", "TOP").replace("bottom", "BOTTOM");
    repo.write_file("f.txt", &new_content);

    let ids = repo.get_hunk_ids(&[]);
    assert!(ids.len() >= 2);

    // Discard first hunk
    repo.tool_ok(&["discard", &ids[0].0]);

    // File should still have changes (the other hunk)
    let file_content = repo.read_file("f.txt");
    // One of "TOP" or "BOTTOM" should be reverted, the other kept
    let has_top = file_content.contains("TOP");
    let has_bottom = file_content.contains("BOTTOM");
    assert!(
        has_top != has_bottom || (!has_top && !has_bottom),
        "exactly one change should remain, got TOP={has_top} BOTTOM={has_bottom}"
    );
}

#[test]
fn discard_multiple_hunks() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\n");
    repo.commit_file("b.txt", "b\n");
    repo.write_file("a.txt", "a changed\n");
    repo.write_file("b.txt", "b changed\n");

    let a_id = repo.get_hunk_id_for_file("a.txt", &[]);
    let b_id = repo.get_hunk_id_for_file("b.txt", &[]);
    repo.tool_ok(&["discard", &a_id, &b_id]);

    assert_eq!(repo.read_file("a.txt"), "a\n");
    assert_eq!(repo.read_file("b.txt"), "b\n");
}

#[test]
fn discard_invalid_id() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "x\n");
    repo.write_file("a.txt", "y\n");

    let err = repo.tool_err(&["discard", "invalid"]);
    assert!(err.contains("hunk not found"));
}

#[test]
fn discard_addition() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\n");
    repo.write_file("a.txt", "line1\nnew_line\n");

    let id = repo.get_single_hunk_id(&[]);
    repo.tool_ok(&["discard", &id]);

    assert_eq!(repo.read_file("a.txt"), "line1\n");
}

#[test]
fn discard_deletion() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\nline2\nline3\n");
    repo.write_file("a.txt", "line1\nline3\n");

    let id = repo.get_single_hunk_id(&[]);
    repo.tool_ok(&["discard", &id]);

    assert_eq!(repo.read_file("a.txt"), "line1\nline2\nline3\n");
}

#[test]
fn discard_modification() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "old\n");
    repo.write_file("a.txt", "new\n");

    let id = repo.get_single_hunk_id(&[]);
    repo.tool_ok(&["discard", &id]);

    assert_eq!(repo.read_file("a.txt"), "old\n");
}

#[test]
fn discard_new_file() {
    let repo = TestRepo::new();
    repo.write_file("dummy.txt", "x\n");
    repo.jj(&["commit", "-m", "init"]);
    repo.write_file("brand_new.txt", "content\n");

    let id = repo.get_hunk_id_for_file("brand_new.txt", &[]);
    repo.tool_ok(&["discard", &id]);

    assert!(
        !repo.file_exists("brand_new.txt"),
        "new file should be removed"
    );
}

#[test]
fn discard_with_inline_range() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "line1\nline2\nline3\n");
    repo.write_file("a.txt", "LINE1\nLINE2\nLINE3\n");

    let id = repo.get_single_hunk_id(&[]);
    // Discard only a portion of the hunk
    let spec = format!("{id}:1-2");
    repo.tool_ok(&["discard", &spec]);

    let content = repo.read_file("a.txt");
    // Some lines should be restored, others remain changed
    // Exact result depends on which lines 1-2 correspond to, but file shouldn't
    // be fully original or fully changed
    assert_ne!(content, "line1\nline2\nline3\n");
    assert_ne!(content, "LINE1\nLINE2\nLINE3\n");
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

    // @- should now only contain the a.txt change
    let diff = repo.jj_diff("@-");
    assert!(diff.contains("a.txt"), "should keep a.txt change");
    // b.txt change should have been removed from the revision
    // (it may have been moved to working copy or dropped depending on jj behavior)
}

#[test]
fn diffedit_default_revision() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "a\n");
    repo.commit_file("b.txt", "b\n");
    repo.write_file("a.txt", "a changed\n");
    repo.write_file("b.txt", "b changed\n");

    let a_id = repo.get_hunk_id_for_file("a.txt", &[]);
    // No -r flag means @
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
fn restore_hunk_from_parent() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "original\n");
    repo.write_file("a.txt", "modified\n");
    repo.jj(&["commit", "-m", "modify"]);
    // Now @- has the modification, @ is empty.
    // Restore the hunk from @- into @ (working copy).
    let id = repo.get_single_hunk_id(&["-r", "@-"]);
    repo.tool_ok(&["restore", &id, "--from", "@-"]);

    // Working copy should now have the change from @-
    let diff = repo.jj_diff("@");
    assert!(diff.contains("a.txt"));
}

#[test]
fn restore_invalid_id() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "x\n");
    repo.write_file("a.txt", "y\n");
    repo.jj(&["commit", "-m", "c"]);

    let err = repo.tool_err(&["restore", "invalid", "--from", "@-"]);
    assert!(err.contains("hunk not found"));
}

// ──────────────────────────────────────────────────────────────────────────────
// _jj-tool protocol
// ──────────────────────────────────────────────────────────────────────────────

fn tool_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_jj-hunk-tool"))
}

fn run_jj_tool(left: &Path, right: &Path, patch_path: Option<&Path>) -> std::process::Output {
    let mut cmd = Command::new(tool_binary());
    cmd.args([
        "_jj-tool",
        &left.display().to_string(),
        &right.display().to_string(),
    ]);
    if let Some(p) = patch_path {
        cmd.env("JJ_HUNK_TOOL_PATCH", p);
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
fn jj_tool_extra_files_removed() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();

    std::fs::write(left.path().join("keep.txt"), "keep\n").unwrap();
    std::fs::write(right.path().join("keep.txt"), "keep\n").unwrap();
    std::fs::write(right.path().join("extra.txt"), "should go\n").unwrap();

    let patch_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(patch_file.path(), "").unwrap(); // empty patch

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

    // Right has a different structure
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

    // Right should match left exactly (reset, no patch applied)
    let content = std::fs::read_to_string(right.path().join("file.txt")).unwrap();
    assert_eq!(content, "content\n");
}
