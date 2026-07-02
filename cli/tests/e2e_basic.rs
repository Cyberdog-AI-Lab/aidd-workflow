//! E2E tests for Scenario 1: bug-fix workflow basic lifecycle.
//!
//! Task progress is driven exclusively through the `run` daemon (autonomous
//! execution mode) — the old manual `start`/`report`/`complete` CLI commands
//! have been removed. `status` still reads directly from the on-disk state,
//! so these tests spin up `workflow-runner run` + `MockWebhook`, drive it via
//! HTTP callbacks, and inspect `status` / the dispatched payloads.
//!
//! Covers:
//!   1-7  status (JSON) reflects the current execution state
//!   1-8  status (table) output contains expected headers and rows
//!   1-9  template vars are expanded in dispatched task prompts

mod helpers;
use helpers::{pick_free_port, wait_until, MockWebhook, TempProject, CONFIG_STANDARD};
use std::time::Duration;

const DISPATCH_TIMEOUT: Duration = Duration::from_secs(5);

// ── 1-7 ───────────────────────────────────────────────────────────────────────

/// `status --format json` must accurately reflect the current task statuses
/// as the daemon dispatches and completes tasks.
#[test]
fn status_json_reflects_current_state() {
    let proj = TempProject::new(CONFIG_STANDARD);
    let webhook = MockWebhook::start();
    let port = pick_free_port();
    let proc = proj.start_run("bug-fix", port, &webhook.url());

    // reproduce and identify have no requires → both dispatched initially.
    webhook.wait_for_n(2, DISPATCH_TIMEOUT);

    proc.complete("reproduce");
    proc.complete("identify");
    // implement becomes dispatchable once both requires are Completed; its
    // dispatch is marked InProgress in the DB *before* the webhook POST is
    // sent, so waiting for it is a safe synchronization point.
    webhook.wait_for_n(3, DISPATCH_TIMEOUT);

    let status = proj.status();

    assert_eq!(status["workflow"], "bug-fix");
    assert!(status["started_at"].as_str().is_some_and(|s| !s.is_empty()));

    let tasks = status["tasks"].as_array().expect("tasks must be an array");
    assert_eq!(tasks.len(), 4);

    let find = |id: &str| -> &serde_json::Value {
        tasks
            .iter()
            .find(|t| t["id"] == id)
            .unwrap_or_else(|| panic!("task '{id}' not found in status"))
    };

    assert_eq!(find("reproduce")["status"], "completed");
    assert_eq!(find("identify")["status"], "completed");
    assert_eq!(find("implement")["status"], "inprogress");
    assert_eq!(find("complete")["status"], "pending");
}

// ── 1-8 ───────────────────────────────────────────────────────────────────────

/// `status --format table` must emit a human-readable ASCII table with the
/// expected column headers and at least one data row.
#[test]
fn status_table_format_includes_header() {
    let proj = TempProject::new(CONFIG_STANDARD);
    let webhook = MockWebhook::start();
    let port = pick_free_port();
    let proc = proj.start_run("bug-fix", port, &webhook.url());

    webhook.wait_for_n(2, DISPATCH_TIMEOUT);
    proc.complete("reproduce");
    // Wait for the report to be applied before reading status: completing
    // "reproduce" alone doesn't trigger a new dispatch (implement still
    // needs "identify"), so synchronize on the DB write via a short poll.
    wait_until(DISPATCH_TIMEOUT, || {
        let status = proj.status();
        status["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t: &serde_json::Value| t["id"] == "reproduce" && t["status"] == "completed")
    });

    let output = proj.run(&["status", "--format", "table"]);
    assert!(output.status.success(), "status --format table must exit 0");

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("TASK ID"), "table must have a TASK ID column");
    assert!(text.contains("TASK"), "table must have a TASK column");
    assert!(text.contains("STATUS"), "table must have a STATUS column");

    assert!(text.contains("reproduce"), "reproduce must appear in table");
    assert!(
        text.contains("completed"),
        "completed status must appear in table"
    );
    assert!(text.contains("identify"), "identify must appear in table");

    assert!(
        text.contains("Session"),
        "table must include the Session line"
    );
    assert!(
        text.contains("Workflow"),
        "table must include the Workflow line"
    );
}

// ── 1-9 ───────────────────────────────────────────────────────────────────────

/// `{{vars.test}}` in a task prompt must be expanded to the configured value
/// by the time the task is dispatched to the webhook.
#[test]
fn template_vars_are_expanded_in_prompt() {
    let proj = TempProject::new(CONFIG_STANDARD);
    let webhook = MockWebhook::start();
    let port = pick_free_port();
    let proc = proj.start_run("bug-fix", port, &webhook.url());

    webhook.wait_for_n(2, DISPATCH_TIMEOUT);
    proc.complete("reproduce");
    proc.complete("identify");
    let dispatched = webhook.wait_for_n(3, DISPATCH_TIMEOUT);

    let implement = dispatched
        .iter()
        .find(|d| d["task_id"] == "implement")
        .expect("implement must have been dispatched");

    let prompt = implement["prompt"]
        .as_str()
        .expect("implement must have a prompt");
    assert!(
        prompt.contains("make test"),
        "{{{{vars.test}}}} must expand to 'make test': got '{prompt}'"
    );
    assert!(
        !prompt.contains("{{vars.test}}"),
        "raw template placeholder must not remain in the prompt"
    );
}
