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
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
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

    /// Retries `workflow-runner <args...>` (typically a `run` thin-client call)
    /// until it either succeeds or fails for a reason other than "daemon
    /// unreachable", tolerating the short window between spawning `serve` in
    /// the background and its HTTP listener accepting connections.
    ///
    /// Only connection failures are retried — a legitimate application-level
    /// error (e.g. an unknown workflow name) is returned immediately.
    pub fn run_retrying(&self, args: &[&str], timeout: Duration) -> Output {
        let deadline = Instant::now() + timeout;
        loop {
            let out = self.run(args);
            let stderr = String::from_utf8_lossy(&out.stderr);
            if out.status.success() || !stderr.contains("is `workflow-runner serve` running?") {
                return out;
            }
            assert!(
                Instant::now() < deadline,
                "workflow-runner {:?} kept failing to reach the daemon within {timeout:?}",
                args
            );
            std::thread::sleep(Duration::from_millis(50));
        }
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

    /// `workflow-runner approve --callback-port <port>` → parsed JSON.
    /// Exercises the CLI wrapper itself (HTTP POST to a running `serve` daemon),
    /// as opposed to `RunningProcess::approve()` which POSTs directly.
    pub fn approve_cli(&self, callback_port: u16) -> serde_json::Value {
        self.assert_ok(&["approve", "--callback-port", &callback_port.to_string()])
    }

    /// `workflow-runner resume --callback-port <port>` → parsed JSON.
    pub fn resume_cli(&self, callback_port: u16) -> serde_json::Value {
        self.assert_ok(&["resume", "--callback-port", &callback_port.to_string()])
    }

    /// `workflow-runner reject <task_id> --callback-port <port>` → parsed JSON.
    pub fn reject_cli(&self, task_id: &str, callback_port: u16) -> serde_json::Value {
        self.assert_ok(&[
            "reject",
            task_id,
            "--callback-port",
            &callback_port.to_string(),
        ])
    }

    /// `workflow-runner reject <task_id> --reason <reason> --callback-port <port>` → parsed JSON.
    pub fn reject_cli_with_reason(
        &self,
        task_id: &str,
        callback_port: u16,
        reason: &str,
    ) -> serde_json::Value {
        self.assert_ok(&[
            "reject",
            task_id,
            "--reason",
            reason,
            "--callback-port",
            &callback_port.to_string(),
        ])
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
pub fn minimal_report() -> serde_json::Value {
    serde_json::json!({ "summary": null })
}

// ── synchronization helpers ───────────────────────────────────────────────────

/// Poll `predicate` every 50ms until it returns true, or panic after `timeout`.
///
/// Useful for waiting on a daemon-processed HTTP callback (e.g. `/complete`,
/// `/report`) to land in the on-disk state before asserting on `status`, when
/// there is no subsequent webhook dispatch to synchronize on instead.
pub fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    loop {
        if predicate() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "wait_until: condition not met within {timeout:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

// ── run subcommand helpers ────────────────────────────────────────────────────

/// Allocates a free TCP port using a global atomic counter.
///
/// Starts from a high base (40000+) and increments monotonically so that
/// parallel tests each receive a unique port without TOCTOU races.  The OS
/// will refuse a bind at process startup if the port happens to be in use
/// (extremely unlikely in the ephemeral range 40000-49999), but the
/// monotonic increment eliminates the common case where two tests race on the
/// same port after both call bind(:0) and release before the child binds.
pub fn pick_free_port() -> u16 {
    use std::sync::atomic::{AtomicU16, Ordering};
    static NEXT_PORT: AtomicU16 = AtomicU16::new(40000);
    NEXT_PORT.fetch_add(1, Ordering::Relaxed)
}

// ── MockWebhook ───────────────────────────────────────────────────────────────

#[derive(Clone)]
struct MockState {
    received: Arc<Mutex<Vec<serde_json::Value>>>,
}

async fn mock_webhook_handler(
    axum::extract::State(s): axum::extract::State<MockState>,
    body: String,
) -> &'static str {
    let json = serde_json::from_str::<serde_json::Value>(&body).unwrap_or(serde_json::Value::Null);
    s.received.lock().unwrap().push(json);
    "ok"
}

/// A minimal axum-based HTTP server that records every incoming POST body as
/// JSON.  Simulates `channels/webhook.ts` in `workflow-runner run` tests.
pub struct MockWebhook {
    /// Port this server is listening on.
    pub port: u16,
    state: MockState,
    /// Keeps the tokio runtime alive; dropping this shuts the server down.
    _rt: tokio::runtime::Runtime,
}

impl MockWebhook {
    /// Bind on a random port and start serving synchronously.
    pub fn start() -> Self {
        let state = MockState {
            received: Arc::new(Mutex::new(Vec::new())),
        };
        let state_for_app = state.clone();

        let rt =
            tokio::runtime::Runtime::new().expect("MockWebhook: failed to create tokio runtime");

        let port = rt.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("MockWebhook: failed to bind listener");
            let port = listener.local_addr().unwrap().port();

            let app = axum::Router::new()
                .route("/", axum::routing::post(mock_webhook_handler))
                .with_state(state_for_app);

            tokio::spawn(async move {
                axum::serve(listener, app).await.ok();
            });

            port
        });

        Self {
            port,
            state,
            _rt: rt,
        }
    }

    /// Base URL of the mock server, e.g. `"http://127.0.0.1:54321"`.
    /// Pass this to `--webhook-url` or `TempProject::start_run`.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Block until at least `n` POST bodies have been received, then return
    /// a snapshot.  Panics if `timeout` elapses before `n` arrive.
    pub fn wait_for_n(&self, n: usize, timeout: Duration) -> Vec<serde_json::Value> {
        let deadline = Instant::now() + timeout;
        loop {
            {
                let received = self.state.received.lock().unwrap();
                if received.len() >= n {
                    return received.clone();
                }
            }
            assert!(
                Instant::now() < deadline,
                "MockWebhook timeout: expected {} dispatches, got {}",
                n,
                self.state.received.lock().unwrap().len()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Snapshot of all received POST bodies.
    pub fn received(&self) -> Vec<serde_json::Value> {
        self.state.received.lock().unwrap().clone()
    }

    /// Number of POST bodies received so far.
    pub fn count(&self) -> usize {
        self.state.received.lock().unwrap().len()
    }

    /// Discard all previously received bodies.
    pub fn clear(&self) {
        self.state.received.lock().unwrap().clear();
    }
}

// ── RunningProcess ────────────────────────────────────────────────────────────

/// A running `workflow-runner serve` background process.
/// Provides helpers for driving workflows via HTTP callbacks.
/// The child process is killed when this value is dropped.
///
/// `workflow_id` holds the "primary" workflow this process was started with
/// (via `TempProject::start_run`); the ambient methods (`complete`, `approve`,
/// etc.) target it implicitly so existing single-workflow tests need no
/// changes. For multi-workflow tests, use the `_for` variants or
/// `start_workflow` to add further workflows to the same daemon.
pub struct RunningProcess {
    child: std::process::Child,
    /// Port that the callback HTTP server is listening on.
    pub callback_port: u16,
    client: reqwest::blocking::Client,
    /// The workflow_id this process was started with (empty if started via
    /// `start_daemon` with no initial workflow).
    pub workflow_id: String,
}

impl RunningProcess {
    fn post(&self, path: &str) {
        self.client
            .post(format!("http://127.0.0.1:{}{}", self.callback_port, path))
            .send()
            .unwrap_or_else(|e| panic!("RunningProcess: failed to POST {path}: {e}"));
    }

    fn post_json(&self, path: &str, body: serde_json::Value) {
        self.client
            .post(format!("http://127.0.0.1:{}{}", self.callback_port, path))
            .json(&body)
            .send()
            .unwrap_or_else(|e| panic!("RunningProcess: failed to POST {path} with body: {e}"));
    }

    /// `POST /run` on this already-running daemon to start an additional
    /// workflow. Returns the newly assigned `workflow_id`.
    ///
    /// Retries briefly to tolerate the short window between spawning `serve`
    /// and its HTTP listener accepting connections.
    pub fn start_workflow(&self, workflow: &str) -> String {
        let url = format!("http://127.0.0.1:{}/run", self.callback_port);
        let body = serde_json::json!({ "workflow": workflow });
        let deadline = Instant::now() + Duration::from_secs(5);
        let resp = loop {
            match self.client.post(&url).json(&body).send() {
                Ok(resp) => break resp,
                Err(e) => {
                    if Instant::now() >= deadline {
                        panic!("RunningProcess: failed to POST /run after retrying: {e}");
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        };
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        assert!(
            status.is_success(),
            "RunningProcess: POST /run for '{workflow}' failed ({status}): {text}"
        );
        let json: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| {
            panic!("RunningProcess: /run response is not valid JSON: {e}\nraw: {text}")
        });
        json["workflow_id"]
            .as_str()
            .unwrap_or_else(|| panic!("RunningProcess: /run response missing workflow_id: {text}"))
            .to_string()
    }

    /// `POST /stop` — asks the daemon to shut down gracefully.
    pub fn stop(&self) {
        self.post("/stop");
    }

    /// `POST /complete/<workflow_id>/<task_id>` for this process's primary workflow.
    pub fn complete(&self, task_id: &str) {
        self.complete_for(&self.workflow_id, task_id);
    }

    /// `POST /complete/<workflow_id>/<task_id>` for an explicit workflow_id.
    pub fn complete_for(&self, workflow_id: &str, task_id: &str) {
        self.post(&format!("/complete/{workflow_id}/{task_id}"));
    }

    /// `POST /report/<workflow_id>/<task_id>` with `{"summary": ...}` body, for
    /// this process's primary workflow.
    pub fn report(&self, task_id: &str, summary: &str) {
        self.report_for(&self.workflow_id, task_id, summary);
    }

    /// `POST /report/<workflow_id>/<task_id>` with `{"summary": ...}` body, for
    /// an explicit workflow_id.
    pub fn report_for(&self, workflow_id: &str, task_id: &str, summary: &str) {
        self.post_json(
            &format!("/report/{workflow_id}/{task_id}"),
            serde_json::json!({ "summary": summary }),
        );
    }

    /// `POST /approve/<workflow_id>` for this process's primary workflow.
    pub fn approve(&self) {
        self.approve_for(&self.workflow_id);
    }

    /// `POST /approve/<workflow_id>` for an explicit workflow_id.
    pub fn approve_for(&self, workflow_id: &str) {
        self.post(&format!("/approve/{workflow_id}"));
    }

    /// `POST /resume/<workflow_id>` for this process's primary workflow.
    pub fn resume(&self) {
        self.resume_for(&self.workflow_id);
    }

    /// `POST /resume/<workflow_id>` for an explicit workflow_id.
    pub fn resume_for(&self, workflow_id: &str) {
        self.post(&format!("/resume/{workflow_id}"));
    }

    /// `POST /reject/<workflow_id>/<task_id>` for this process's primary workflow.
    pub fn reject(&self, task_id: &str) {
        self.reject_for(&self.workflow_id, task_id);
    }

    /// `POST /reject/<workflow_id>/<task_id>` for an explicit workflow_id.
    pub fn reject_for(&self, workflow_id: &str, task_id: &str) {
        self.post(&format!("/reject/{workflow_id}/{task_id}"));
    }

    /// `POST /reject/<workflow_id>/<task_id>` with `{"reason": ...}` body, for
    /// this process's primary workflow.
    pub fn reject_with_reason(&self, task_id: &str, reason: &str) {
        self.reject_with_reason_for(&self.workflow_id, task_id, reason);
    }

    /// `POST /reject/<workflow_id>/<task_id>` with `{"reason": ...}` body, for
    /// an explicit workflow_id.
    pub fn reject_with_reason_for(&self, workflow_id: &str, task_id: &str, reason: &str) {
        self.post_json(
            &format!("/reject/{workflow_id}/{task_id}"),
            serde_json::json!({ "reason": reason }),
        );
    }

    /// `POST /pause/<workflow_id>/<task_id>` for this process's primary workflow.
    pub fn pause(&self, task_id: &str) {
        self.pause_for(&self.workflow_id, task_id);
    }

    /// `POST /pause/<workflow_id>/<task_id>` for an explicit workflow_id.
    pub fn pause_for(&self, workflow_id: &str, task_id: &str) {
        self.post(&format!("/pause/{workflow_id}/{task_id}"));
    }

    /// `POST /pause/<workflow_id>/<task_id>` with `{"reason": ...}` body, for
    /// this process's primary workflow.
    pub fn pause_with_reason(&self, task_id: &str, reason: &str) {
        self.pause_with_reason_for(&self.workflow_id, task_id, reason);
    }

    /// `POST /pause/<workflow_id>/<task_id>` with `{"reason": ...}` body, for
    /// an explicit workflow_id.
    pub fn pause_with_reason_for(&self, workflow_id: &str, task_id: &str, reason: &str) {
        self.post_json(
            &format!("/pause/{workflow_id}/{task_id}"),
            serde_json::json!({ "reason": reason }),
        );
    }

    /// Non-blocking check of whether the process has exited.
    /// Returns `Some(status)` if it has, `None` if it is still running.
    pub fn try_wait(&mut self) -> Option<std::process::ExitStatus> {
        self.child
            .try_wait()
            .unwrap_or_else(|e| panic!("RunningProcess: error checking exit status: {e}"))
    }

    /// Block until the process exits and return its exit status.
    /// Kills the process and panics if `timeout` elapses first.
    ///
    /// Note: `serve` no longer exits automatically when its tracked workflows
    /// complete — only `POST /stop` or killing the process ends it. Use this
    /// after calling `stop()`, or use `wait_workflow_completed` to assert a
    /// workflow finished without expecting the daemon itself to exit.
    pub fn wait_exit(&mut self, timeout: Duration) -> std::process::ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return status,
                Ok(None) => {}
                Err(e) => panic!("RunningProcess: error checking exit status: {e}"),
            }
            if Instant::now() >= deadline {
                self.child.kill().ok();
                panic!("RunningProcess: timeout waiting for workflow-runner serve to exit");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for RunningProcess {
    fn drop(&mut self) {
        self.child.kill().ok();
        self.child.wait().ok();
    }
}

// ── TempProject run extensions ────────────────────────────────────────────────

impl TempProject {
    /// Spawn `workflow-runner serve` in the background, then start `workflow`
    /// on it via `POST /run`. The returned `RunningProcess.workflow_id` is set
    /// to the newly assigned id, so `complete()`/`approve()`/etc. work without
    /// an explicit workflow_id — existing single-workflow tests need no changes.
    ///
    /// Use `pick_free_port()` for `callback_port` and `MockWebhook::url()` for
    /// `webhook_url` to avoid port conflicts between parallel tests.
    pub fn start_run(
        &self,
        workflow: &str,
        callback_port: u16,
        webhook_url: &str,
    ) -> RunningProcess {
        let mut proc = self.start_daemon(callback_port, webhook_url);
        proc.workflow_id = proc.start_workflow(workflow);
        proc
    }

    /// Spawn `workflow-runner serve` in the background with a specific callback
    /// port and webhook URL, without starting any workflow. Use
    /// `RunningProcess::start_workflow` to add one (or several) afterward.
    /// Useful for multi-workflow tests and negative tests (e.g. an unknown
    /// workflow name).
    pub fn start_daemon(&self, callback_port: u16, webhook_url: &str) -> RunningProcess {
        let child = Command::new(binary_path())
            .arg("--cwd")
            .arg(self.path())
            .arg("serve")
            .args(["--callback-port", &callback_port.to_string()])
            .args(["--webhook-url", webhook_url])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|e| panic!("start_daemon: failed to spawn workflow-runner: {e}"));
        RunningProcess {
            child,
            callback_port,
            client: reqwest::blocking::Client::new(),
            workflow_id: String::new(),
        }
    }

    /// Spawn `workflow-runner` with arbitrary args in the background and return
    /// the raw `Child`.  The caller must kill and wait on the process.
    pub fn run_background(&self, args: &[&str]) -> std::process::Child {
        Command::new(binary_path())
            .arg("--cwd")
            .arg(self.path())
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|e| panic!("run_background: failed to spawn workflow-runner: {e}"))
    }
}

// ── Workflow-completion polling ───────────────────────────────────────────────

/// Polls `status --workflow-id <id>` until it fails (the workflow is no longer
/// active/paused/awaiting_approval — i.e. it completed and was cleared from the
/// store), or panics after `timeout`.
///
/// Use this instead of `RunningProcess::wait_exit` to assert a workflow
/// finished: `serve` no longer exits automatically when its tracked workflows
/// complete, so process exit is not a valid completion signal anymore.
pub fn wait_workflow_completed(proj: &TempProject, workflow_id: &str, timeout: Duration) {
    // `--workflow-id` is a global flag defined on the top-level `Cli` struct,
    // so it must precede the subcommand: `--workflow-id <id> status`, not
    // `status --workflow-id <id>` (the latter is rejected by clap outright,
    // which would make this check vacuously true on the first poll).
    wait_until(timeout, || {
        !proj
            .run(&["--workflow-id", workflow_id, "status"])
            .status
            .success()
    });
}
