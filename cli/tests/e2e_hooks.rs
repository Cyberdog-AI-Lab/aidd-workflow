//! E2E tests for Scenario 4: hook enforcement (pre-edit, pre-bash, post-edit).
//!
//! Covers:
//!   4-1  pre-edit allows a file within the task's outputs
//!   4-2  pre-edit asks when a file is outside outputs
//!   4-3  pre-edit blocks a file that matches deny.files
//!   4-4  deny.files takes priority over outputs (block beats ask)
//!   4-5  pre-bash blocks a command matching deny.commands
//!   4-6  pre-bash allows a command not in deny.commands
//!   4-7  post-edit returns empty output when config.yml is valid
//!   4-8  post-edit emits a schema warning when config.yml is invalid
//!   4-9  hooks are no-ops when no workflow is active

mod helpers;
use helpers::{pick_free_port, MockWebhook, RunningProcess, TempProject};
use std::time::Duration;

/// Custom config with a single task that has both `outputs` and `deny`.
/// Used for all pre-edit and pre-bash tests.
const CONFIG_GUARDED: &str = r#"
workflows:
  guarded:
    name: Guarded workflow
    tasks:
      - id: implement
        task: Implement
        prompt: Do the implementation work.
        outputs:
          - "src/**"
          - "tests/**"
        deny:
          files:
            - "docs/specs/**"
          commands:
            - "git push"
"#;

/// Invalid config — empty task list triggers a validation error in load_config.
const CONFIG_INVALID: &str = r#"
workflows:
  bad:
    name: Bad workflow
    tasks: []
"#;

// ── local helpers ─────────────────────────────────────────────────────────────

/// Build the hook stdin JSON for a pre-edit / post-edit event.
/// `file_path` is interpreted relative to the project root; the absolute path
/// is embedded in the JSON so the handler can strip the cwd prefix correctly.
fn edit_json(proj: &TempProject, rel_path: &str) -> String {
    let abs = proj.path().join(rel_path);
    serde_json::json!({
        "cwd": proj.path().to_string_lossy(),
        "tool_input": { "file_path": abs.to_string_lossy() }
    })
    .to_string()
}

/// Build the hook stdin JSON for a pre-bash / post-bash event.
fn bash_json(proj: &TempProject, command: &str) -> String {
    serde_json::json!({
        "cwd": proj.path().to_string_lossy(),
        "tool_input": { "command": command }
    })
    .to_string()
}

/// Start the guarded workflow via the `run` daemon and wait for `implement`
/// to be dispatched, which marks it InProgress in the DB (hooks only fire
/// for InProgress tasks). The mock webhook is torn down once dispatch is
/// confirmed; the caller must keep the returned `RunningProcess` alive for
/// the duration of the test (state persists on disk regardless).
fn activate_implement(proj: &TempProject) -> RunningProcess {
    let webhook = MockWebhook::start();
    let cb_port = pick_free_port();
    let proc = proj.start_run("guarded", cb_port, &webhook.url());
    webhook.wait_for_n(1, Duration::from_secs(5));
    proc
}

/// Parse `output.stdout` as a JSON Value; panics with stdout content on failure.
fn parse_hook_stdout(output: &std::process::Output) -> serde_json::Value {
    let raw = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(raw.trim())
        .unwrap_or_else(|e| panic!("hook stdout is not valid JSON: {e}\nraw: {raw}"))
}

// ── 4-1 ───────────────────────────────────────────────────────────────────────

/// A file that matches the task's `outputs` pattern must be silently allowed:
/// the hook produces no output (empty stdout).
#[test]
fn pre_edit_allows_within_outputs() {
    let proj = TempProject::new(CONFIG_GUARDED);
    let _proc = activate_implement(&proj);

    let output = proj.hook("pre-edit", &edit_json(&proj, "src/main.rs"));

    assert!(output.status.success(), "hook must exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().is_empty(),
        "hook must produce no output for an allowed file: got '{stdout}'"
    );
}

// ── 4-2 ───────────────────────────────────────────────────────────────────────

/// A file outside the `outputs` allowlist must trigger a `decision: "ask"` response.
#[test]
fn pre_edit_asks_outside_outputs() {
    let proj = TempProject::new(CONFIG_GUARDED);
    let _proc = activate_implement(&proj);

    let output = proj.hook("pre-edit", &edit_json(&proj, "README.md"));

    assert!(output.status.success());
    let v = parse_hook_stdout(&output);
    assert_eq!(
        v["decision"], "ask",
        "file outside outputs must produce decision:ask"
    );
    assert!(
        v["reason"]
            .as_str()
            .is_some_and(|r| r.contains("README.md")),
        "reason must mention the file path"
    );
}

// ── 4-3 ───────────────────────────────────────────────────────────────────────

/// A file matching `deny.files` must trigger a `decision: "block"` response.
#[test]
fn pre_edit_blocks_deny_file() {
    let proj = TempProject::new(CONFIG_GUARDED);
    let _proc = activate_implement(&proj);

    let output = proj.hook("pre-edit", &edit_json(&proj, "docs/specs/design.md"));

    assert!(output.status.success());
    let v = parse_hook_stdout(&output);
    assert_eq!(
        v["decision"], "block",
        "deny.files match must produce decision:block"
    );
    let reason = v["reason"].as_str().expect("reason must be a string");
    assert!(
        reason.contains("docs/specs/design.md"),
        "reason must mention the blocked file: got '{reason}'"
    );
}

// ── 4-4 ───────────────────────────────────────────────────────────────────────

/// When a file is both outside `outputs` AND matches `deny.files`, the result
/// must be `block` (not `ask`).  deny.files is evaluated first.
#[test]
fn pre_edit_deny_takes_priority_over_outputs() {
    let proj = TempProject::new(CONFIG_GUARDED);
    let _proc = activate_implement(&proj);

    // docs/specs/** is in deny.files; it is also outside src/** and tests/**
    let output = proj.hook("pre-edit", &edit_json(&proj, "docs/specs/api.yaml"));

    assert!(output.status.success());
    let v = parse_hook_stdout(&output);
    assert_eq!(
        v["decision"], "block",
        "deny.files must win over outputs: expected block, not ask"
    );
}

// ── 4-5 ───────────────────────────────────────────────────────────────────────

/// A command that contains the denied substring must produce `decision: "block"`.
#[test]
fn pre_bash_blocks_denied_command() {
    let proj = TempProject::new(CONFIG_GUARDED);
    let _proc = activate_implement(&proj);

    // deny.commands = ["git push"] — substring match
    let output = proj.hook("pre-bash", &bash_json(&proj, "git push origin main"));

    assert!(output.status.success());
    let v = parse_hook_stdout(&output);
    assert_eq!(
        v["decision"], "block",
        "denied command must produce decision:block"
    );
    let reason = v["reason"].as_str().expect("reason must be a string");
    assert!(
        reason.contains("git push"),
        "reason must mention the blocked command: got '{reason}'"
    );
}

// ── 4-6 ───────────────────────────────────────────────────────────────────────

/// A command not present in `deny.commands` must be silently allowed
/// (empty stdout).
#[test]
fn pre_bash_allows_non_denied_command() {
    let proj = TempProject::new(CONFIG_GUARDED);
    let _proc = activate_implement(&proj);

    let output = proj.hook("pre-bash", &bash_json(&proj, "cargo test --all"));

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().is_empty(),
        "allowed command must produce no output: got '{stdout}'"
    );
}

// ── 4-7 ───────────────────────────────────────────────────────────────────────

/// When config.yml is valid, the post-edit hook must produce no output.
#[test]
fn post_edit_valid_config_returns_empty() {
    let proj = TempProject::new(CONFIG_GUARDED);

    let output = proj.hook("post-edit", &edit_json(&proj, ".workflow/config.yml"));

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().is_empty(),
        "post-edit hook must be silent when config.yml is valid: got '{stdout}'"
    );
}

// ── 4-8 ───────────────────────────────────────────────────────────────────────

/// When config.yml contains a validation error, the post-edit hook must emit
/// a `[SCHEMA WARNING]` message to stdout (exit 0 — hooks never crash).
#[test]
fn post_edit_invalid_config_emits_warning() {
    let proj = TempProject::new(CONFIG_GUARDED);

    // Overwrite config.yml with an invalid definition (empty task list).
    proj.write_workflow_config(CONFIG_INVALID);

    let output = proj.hook("post-edit", &edit_json(&proj, ".workflow/config.yml"));

    assert!(
        output.status.success(),
        "hook must exit 0 even when config.yml is invalid"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("SCHEMA WARNING"),
        "post-edit hook must emit a schema warning for an invalid config: got '{stdout}'"
    );
}

// ── 4-9 ───────────────────────────────────────────────────────────────────────

/// When no workflow is active, all hooks must be no-ops (empty stdout, exit 0).
/// This ensures hooks do not interfere with work done outside a managed workflow.
#[test]
fn hook_no_active_workflow_returns_empty() {
    // Project exists but no `start` has been called — DB is absent / empty.
    let proj = TempProject::new(CONFIG_GUARDED);

    for event in &["pre-edit", "pre-bash", "post-bash"] {
        let stdin = if *event == "pre-bash" {
            bash_json(&proj, "git push origin main")
        } else {
            edit_json(&proj, "src/main.rs")
        };

        let output = proj.hook(event, &stdin);
        assert!(
            output.status.success(),
            "hook '{event}' must exit 0 with no active workflow"
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.trim().is_empty(),
            "hook '{event}' must produce no output with no active workflow: got '{stdout}'"
        );
    }
}
