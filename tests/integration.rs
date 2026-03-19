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
}

#[test]
fn test_hunks_lists_changes() {
    let repo = TestRepo::new();
    repo.write_file("a.txt", "line1\n");
    repo.jj(&["commit", "-m", "initial"]);
    repo.write_file("a.txt", "line1\nadded\n");

    let output = repo.tool(&["hunks"]).unwrap();
    assert!(output.contains("a.txt"), "should list a.txt hunk");
    assert!(output.contains("+1 -0"), "should show +1 -0");
    assert!(output.contains("+added"), "should preview added line");
}

#[test]
fn test_hunks_full_mode() {
    let repo = TestRepo::new();
    repo.write_file("a.txt", "line1\n");
    repo.jj(&["commit", "-m", "initial"]);
    repo.write_file("a.txt", "line1\nadded\n");

    let output = repo.tool(&["hunks", "--full"]).unwrap();
    // Full mode shows line numbers
    assert!(output.contains("1:"), "should show line numbers");
}

#[test]
fn test_hunks_file_filter() {
    let repo = TestRepo::new();
    repo.write_file("a.txt", "line1\n");
    repo.write_file("b.txt", "line2\n");
    repo.jj(&["commit", "-m", "initial"]);
    repo.write_file("a.txt", "line1\nchanged-a\n");
    repo.write_file("b.txt", "line2\nchanged-b\n");

    let output = repo.tool(&["hunks", "--file", "a.txt"]).unwrap();
    assert!(output.contains("a.txt"), "should show a.txt");
    assert!(!output.contains("b.txt"), "should not show b.txt");
}

#[test]
fn test_show_hunk() {
    let repo = TestRepo::new();
    repo.write_file("a.txt", "line1\n");
    repo.jj(&["commit", "-m", "initial"]);
    repo.write_file("a.txt", "line1\nadded\n");

    let hunks_output = repo.tool(&["hunks"]).unwrap();
    let id = hunks_output.split_whitespace().next().unwrap();

    let show_output = repo.tool(&["show", id]).unwrap();
    assert!(show_output.contains("@@"), "should show hunk header");
    assert!(show_output.contains("1:"), "should show line numbers");
}

#[test]
fn test_patch_output() {
    let repo = TestRepo::new();
    repo.write_file("a.txt", "line1\n");
    repo.jj(&["commit", "-m", "initial"]);
    repo.write_file("a.txt", "line1\nadded\n");

    let hunks_output = repo.tool(&["hunks"]).unwrap();
    let id = hunks_output.split_whitespace().next().unwrap();

    let patch = repo.tool(&["patch", id]).unwrap();
    assert!(patch.contains("--- a/a.txt"), "should have --- header");
    assert!(patch.contains("+++ b/a.txt"), "should have +++ header");
    assert!(patch.contains("+added"), "should include added line");
}

#[test]
fn test_patch_reverse() {
    let repo = TestRepo::new();
    repo.write_file("a.txt", "line1\n");
    repo.jj(&["commit", "-m", "initial"]);
    repo.write_file("a.txt", "line1\nadded\n");

    let hunks_output = repo.tool(&["hunks"]).unwrap();
    let id = hunks_output.split_whitespace().next().unwrap();

    // --reverse affects line-range context handling (for partial hunks).
    // For whole hunks, the output is the same patch; reversal is done by
    // `patch -p1 --reverse` at apply time.
    let patch = repo.tool(&["patch", "--reverse", id]).unwrap();
    assert!(patch.contains("a.txt"), "reverse should contain file ref");
    assert!(
        patch.contains("+added"),
        "whole-hunk reverse keeps original lines"
    );
}

#[test]
fn test_commit_single_hunk() {
    let repo = TestRepo::new();
    repo.write_file("a.txt", "line1\n");
    repo.write_file("b.txt", "line2\n");
    repo.jj(&["commit", "-m", "initial"]);
    repo.write_file("a.txt", "line1\nadded-a\n");
    repo.write_file("b.txt", "line2\nadded-b\n");

    let hunks_output = repo.tool(&["hunks"]).unwrap();
    // Find the a.txt hunk ID
    let a_id = hunks_output
        .lines()
        .find(|l| l.contains("a.txt"))
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap();

    repo.tool(&["commit", a_id, "-m", "add a only"]).unwrap();

    // The parent commit should have only a.txt change
    let parent_diff = repo.jj(&["diff", "--git", "-r", "@-"]);
    assert!(parent_diff.contains("a.txt"), "parent should change a.txt");
    assert!(
        !parent_diff.contains("b.txt"),
        "parent should not change b.txt"
    );
}

#[test]
fn test_jj_tool_protocol() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();

    // Set up left (base) state
    std::fs::write(left.path().join("file.txt"), "hello\n").unwrap();

    // Set up right (current) state with extra file
    std::fs::write(right.path().join("file.txt"), "hello\n").unwrap();
    std::fs::write(right.path().join("extra.txt"), "should be removed\n").unwrap();

    // Write a patch
    let patch_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        patch_file.path(),
        "--- a/file.txt\n+++ b/file.txt\n@@ -1 +1,2 @@\n hello\n+world\n",
    )
    .unwrap();

    let binary = PathBuf::from(env!("CARGO_BIN_EXE_jj-hunk-tool"));
    let output = Command::new(&binary)
        .args([
            "_jj-tool",
            &left.path().display().to_string(),
            &right.path().display().to_string(),
        ])
        .env("JJ_HUNK_TOOL_PATCH", patch_file.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "tool protocol failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify right was reset and patch applied
    assert!(
        !right.path().join("extra.txt").exists(),
        "extra file should be removed"
    );
    let content = std::fs::read_to_string(right.path().join("file.txt")).unwrap();
    assert_eq!(content, "hello\nworld\n", "patch should be applied");
}

#[test]
fn test_hunk_id_stability() {
    let repo = TestRepo::new();
    repo.write_file("a.txt", "line1\n");
    repo.jj(&["commit", "-m", "initial"]);
    repo.write_file("a.txt", "line1\nadded\n");

    let output1 = repo.tool(&["hunks"]).unwrap();
    let id1 = output1.split_whitespace().next().unwrap().to_string();

    let output2 = repo.tool(&["hunks"]).unwrap();
    let id2 = output2.split_whitespace().next().unwrap().to_string();

    assert_eq!(id1, id2, "hunk IDs should be stable across calls");
}

#[test]
fn test_discard_hunk() {
    let repo = TestRepo::new();
    repo.write_file("a.txt", "line1\n");
    repo.jj(&["commit", "-m", "initial"]);
    repo.write_file("a.txt", "line1\nadded\n");

    let hunks_output = repo.tool(&["hunks"]).unwrap();
    let id = hunks_output.split_whitespace().next().unwrap();

    repo.tool(&["discard", id]).unwrap();

    let content = repo.read_file("a.txt");
    assert_eq!(content, "line1\n", "added line should be removed");
}
