//! Shared helpers for E2E integration tests.
//!
//! # Usage
//!
//! Each integration test file should declare this module at the top:
//!
//! ```rust,ignore
//! mod helpers;
//! use helpers::TempProject;
//! ```
//!
//! Then create a project and run commands:
//!
//! ```rust,ignore
//! let proj = TempProject::new(helpers::CONFIG_STANDARD);
//! let out  = proj.start("bug-fix");
//! assert_eq!(out["status"], "started");
//! ```

#![allow(dead_code)]

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

// ── Binary resolution ─────────────────────────────────────────────────────────

/// Returns the path to the compiled `workflow-runner` binary.
///
/// Integration test binaries live in `target/{profile}/deps/`; the main binary
/// lives one level above in `target/{profile}/`.  We resolve the path at
/// runtime so the same helper works for both `cargo test` (debug) and
/// `cargo test --release` (release) profiles.
fn binary_path() -> PathBuf {
    // current_exe() points to the test binary itself, e.g.
    //   …/target/debug/deps/e2e_basic-<hash>
    let mut path = std::env::current_exe().expect("cannot resolve current_exe() for binary_path()");
    path.pop(); // strip the test binary filename
    if path.ends_with("deps") {
        path.pop(); // ascend from target/{profile}/deps/ → target/{profile}/
    }
    let bin = path.join("workflow-runner");
    assert!(
        bin.exists(),
        "workflow-runner binary not found at {}.\nRun `cargo build` before executing tests.",
        bin.display()
    );
    bin
}

// ── Fixture YAML ──────────────────────────────────────────────────────────────

/// Standard three-workflow config: `bug-fix`, `feature` (approval), `release` (agents).
///
/// This mirrors the structure of the project's own `.workflow/config.yml` and
/// covers the most common combinations of fields used in E2E tests.
pub const CONFIG_STANDARD: &str = r#"
vars:
  test: make test
  lint: make lint
  build: make build

workflows:
  bug-fix:
    name: Bug Fix Flow
    tasks:
      - id: reproduce
        task: Reproduce the bug
        prompt: Reproduce the bug and document the steps.

      - id: identify
        task: Identify root cause
        prompt: Identify the root cause.

      - id: implement
        task: Fix the bug
        prompt: "Fix the bug. Then run {{vars.test}}."
        outputs:
          - "src/**"
          - "tests/**"
        requires: [reproduce, identify]

      - id: complete
        task: Confirm completion
        prompt: Confirm everything is complete.
        requires: [implement]

  feature:
    name: Feature Development Flow
    tasks:
      - id: design
        task: Write design doc
        prompt: Write the design document.
        outputs:
          - "docs/**"
          - "/.*\\.md$/"
        approval: true

      - id: implement
        task: Implement
        prompt: "Implement the feature. Run {{vars.test}} and {{vars.lint}}."
        outputs:
          - "src/**"
          - "tests/**"
        deny:
          files:
            - "docs/specs/**"
        requires: [design]

      - id: complete
        task: Confirm completion
        prompt: Confirm everything is complete.
        requires: [implement]

  release:
    name: Release Flow
    tasks:
      - id: design
        task: Write design doc
        prompt: Write the design document.
        outputs:
          - "docs/**"
        approval: true

      - id: implement
        task: Implement
        prompt: "Implement. Run {{vars.build}} to verify."
        outputs:
          - "src/**"
          - "tests/**"
        requires: [design]

      - id: quality-check
        task: Quality check
        requires: [implement]
        agents:
          - run-test
          - run-lint

      - id: complete
        task: Confirm completion
        prompt: Confirm design, implementation, and quality check are all done.
        requires: [quality-check]
        approval: true
"#;

/// Minimal single-task workflow — useful for smoke tests and error-case setups.
pub const CONFIG_MINIMAL: &str = r#"
workflows:
  simple:
    name: Simple Flow
    tasks:
      - id: only-task
        task: The only task
        prompt: Do the thing.
"#;

// ── TempProject ───────────────────────────────────────────────────────────────

/// A self-contained temporary project directory for E2E tests.
///
/// `workflow-runner --cwd <dir>` is prepended automatically to every command,
/// so tests never need to specify `--cwd` explicitly.
///
/// The underlying `TempDir` is removed when the `TempProject` is dropped.
pub struct TempProject {
    /// The underlying temporary directory (cleaned up on `Drop`).
    pub dir: TempDir,
}

impl TempProject {
    // ── Constructors ──────────────────────────────────────────────────────────

    /// Create a new project with the given `config.yml` content written to
    /// `.workflow/config.yml`.
    pub fn new(config_yaml: &str) -> Self {
        let dir = tempfile::tempdir().expect("failed to create TempDir");
        let project = Self { dir };
        project.write_workflow_config(config_yaml);
        project
    }

    /// Create a new project with *no* `config.yml`.
    /// Useful for testing error messages when the config is missing.
    pub fn empty() -> Self {
        let dir = tempfile::tempdir().expect("failed to create TempDir");
        Self { dir }
    }

    // ── Path helpers ──────────────────────────────────────────────────────────

    /// The project root path.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    // ── File helpers ──────────────────────────────────────────────────────────

    /// Write (or overwrite) `.workflow/config.yml`.
    pub fn write_workflow_config(&self, yaml: &str) {
        self.write_file(".workflow/config.yml", yaml);
    }

    /// Write (or overwrite) any file relative to the project root.
    /// Parent directories are created as needed.
    pub fn write_file(&self, relative_path: &str, content: &str) {
        let full = self.path().join(relative_path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|e| panic!("failed to create directory {}: {e}", parent.display()));
        }
        std::fs::write(&full, content)
            .unwrap_or_else(|e| panic!("failed to write {relative_path}: {e}"));
    }

    // ── Process execution ─────────────────────────────────────────────────────

    /// Run `workflow-runner --cwd <project> <args...>` and return the raw output.
    pub fn run(&self, args: &[&str]) -> Output {
        self.run_with_stdin(args, "")
    }

    /// Run with the given stdin string and return the raw output.
    ///
    /// Stdin is written from a background thread to avoid blocking on large
    /// payloads while the child is still producing output.
    pub fn run_with_stdin(&self, args: &[&str], stdin: &str) -> Output {
        let mut cmd = Command::new(binary_path());
        cmd.arg("--cwd").arg(self.path());
        cmd.args(args);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .unwrap_or_else(|e| panic!("failed to spawn workflow-runner: {e}"));

        // Feed stdin from a separate thread so the child can drain its stdout
        // and stderr without blocking, preventing pipe-buffer deadlocks.
        if !stdin.is_empty() {
            let mut stdin_handle = child.stdin.take().expect("stdin pipe not captured");
            let bytes = stdin.as_bytes().to_vec();
            std::thread::spawn(move || {
                stdin_handle.write_all(&bytes).ok();
                // Handle is dropped here, closing the pipe.
            });
        }

        child
            .wait_with_output()
            .expect("failed to wait for workflow-runner")
    }

    // ── Assertion helpers ─────────────────────────────────────────────────────

    /// Assert exit code 0 and parse stdout as JSON.
    ///
    /// # Panics
    /// - If the process exits with a non-zero code.
    /// - If stdout is not valid JSON.
    pub fn assert_ok(&self, args: &[&str]) -> serde_json::Value {
        self.assert_ok_with_stdin(args, "")
    }

    /// `assert_ok` with stdin data.
    pub fn assert_ok_with_stdin(&self, args: &[&str], stdin: &str) -> serde_json::Value {
        let output = self.run_with_stdin(args, stdin);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "workflow-runner {:?} exited with status {}\nstdout: {stdout}\nstderr: {stderr}",
            args,
            output.status,
        );
        let trimmed = stdout.trim();
        serde_json::from_str(trimmed)
            .unwrap_or_else(|e| panic!("stdout is not valid JSON: {e}\nraw output:\n{trimmed}"))
    }

    /// Assert exit code != 0 and return the stderr as a `String`.
    ///
    /// # Panics
    /// If the process exits with code 0.
    pub fn assert_err(&self, args: &[&str]) -> String {
        self.assert_err_with_stdin(args, "")
    }

    /// `assert_err` with stdin data.
    pub fn assert_err_with_stdin(&self, args: &[&str], stdin: &str) -> String {
        let output = self.run_with_stdin(args, stdin);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            !output.status.success(),
            "workflow-runner {:?} was expected to fail but exited 0\nstdout: {stdout}",
            args,
        );
        String::from_utf8_lossy(&output.stderr).into_owned()
    }

    // ── Workflow shortcut helpers ─────────────────────────────────────────────

    /// `workflow-runner start <workflow>` → parsed JSON.
    pub fn start(&self, workflow: &str) -> serde_json::Value {
        self.assert_ok(&["start", workflow])
    }

    /// `workflow-runner next` → parsed JSON.
    pub fn next(&self) -> serde_json::Value {
        self.assert_ok(&["next"])
    }

    /// `workflow-runner --workflow-id <id> next` → parsed JSON.
    pub fn next_with_id(&self, workflow_id: &str) -> serde_json::Value {
        self.assert_ok(&["--workflow-id", workflow_id, "next"])
    }

    /// `workflow-runner complete <task_id>` → parsed JSON.
    pub fn complete(&self, task_id: &str) -> serde_json::Value {
        self.assert_ok(&["complete", task_id])
    }

    /// `workflow-runner --workflow-id <id> complete <task_id>` → parsed JSON.
    pub fn complete_with_id(&self, workflow_id: &str, task_id: &str) -> serde_json::Value {
        self.assert_ok(&["--workflow-id", workflow_id, "complete", task_id])
    }

    /// `workflow-runner report` with the given JSON body → parsed JSON.
    pub fn report(&self, body: &serde_json::Value) -> serde_json::Value {
        let json = serde_json::to_string(body).expect("failed to serialize report body");
        self.assert_ok_with_stdin(&["report"], &json)
    }

    /// `workflow-runner reject <task_id>` → parsed JSON.
    pub fn reject(&self, task_id: &str) -> serde_json::Value {
        self.assert_ok(&["reject", task_id])
    }

    /// `workflow-runner reject <task_id> --reason <reason>` → parsed JSON.
    pub fn reject_with_reason(&self, task_id: &str, reason: &str) -> serde_json::Value {
        self.assert_ok(&["reject", task_id, "--reason", reason])
    }

    /// `workflow-runner resume` → parsed JSON.
    pub fn resume(&self) -> serde_json::Value {
        self.assert_ok(&["resume"])
    }

    /// `workflow-runner status` → parsed JSON.
    pub fn status(&self) -> serde_json::Value {
        self.assert_ok(&["status"])
    }

    /// `workflow-runner validate` → parsed JSON.
    pub fn validate(&self) -> serde_json::Value {
        self.assert_ok(&["validate"])
    }

    /// `workflow-runner list` → parsed JSON array.
    pub fn list(&self) -> serde_json::Value {
        self.assert_ok(&["list"])
    }

    /// `workflow-runner hook <event_type>` with the given stdin JSON → raw `Output`.
    ///
    /// Hooks return an empty body on success (no-op) or a JSON decision object.
    /// Use the raw `Output` so callers can inspect stdout / stderr / exit code freely.
    pub fn hook(&self, event_type: &str, stdin_json: &str) -> Output {
        self.run_with_stdin(&["hook", event_type], stdin_json)
    }
}

// ── Hook stdin JSON builders ──────────────────────────────────────────────────

/// Build a `pre-edit` / `post-edit` hook stdin JSON for the given file path.
///
/// The `cwd` field is set to `project_root` so `workflow-runner hook` can
/// resolve the active workflow without `--cwd`.
pub fn hook_edit_json(project_root: &Path, file_path: &str) -> String {
    serde_json::json!({
        "cwd": project_root.to_string_lossy(),
        "tool_input": { "file_path": file_path }
    })
    .to_string()
}

/// Build a `pre-bash` / `post-bash` hook stdin JSON for the given command.
pub fn hook_bash_json(project_root: &Path, command: &str) -> String {
    serde_json::json!({
        "cwd": project_root.to_string_lossy(),
        "tool_input": { "command": command }
    })
    .to_string()
}

// ── Report input builder ──────────────────────────────────────────────────────

/// Build a minimal valid `report` stdin JSON for `task_id`.
///
/// `session_id` is required by the protocol but not validated at runtime,
/// so a fixed placeholder value is used.
pub fn minimal_report(task_id: &str) -> serde_json::Value {
    serde_json::json!({
        "session_id": "test-session",
        "task_id":    task_id,
        "action_index": 0,
        "action_type": "prompt",
        "exit_code": 0,
        "stdout": null
    })
}
