//! E2E tests for Scenario 2: approval / reject flow.
//!
//! These tests drive a `workflow-runner run` daemon and exercise the new
//! `approve` / `reject` CLI subcommands themselves (HTTP clients that POST to
//! the daemon's callback server). The daemon's own HTTP handling for
//! /approve, /resume, and /reject/:id is covered independently in
//! `e2e_run.rs` via raw HTTP (`RunningProcess::approve`/`reject`); this file
//! confirms the CLI wrappers actually reach the daemon and produce the same
//! effect.
//!
//! Covers:
//!   2-1  `approve` CLI advances an awaiting_approval workflow and expands template vars
//!   2-2  `reject` CLI with --reason re-dispatches the rejected task
//!   2-3  `reject` CLI outside awaiting_approval is a silent no-op (no redispatch)
//!   2-4  `approve` CLI on the final approval-gated task completes the workflow

mod helpers;
use helpers::{pick_free_port, MockWebhook, TempProject, CONFIG_STANDARD};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);

/// Two-task workflow whose last task requires sign-off (approval: true).
const CONFIG_SIGN_OFF: &str = r#"
workflows:
  sign-off-flow:
    name: Sign-off Flow
    tasks:
      - id: work
        task: Do the work
        prompt: Do the work.
      - id: sign-off
        task: Sign off
        prompt: Sign off on the completed work.
        requires: [work]
        approval: true
"#;

// ── 2-1 ───────────────────────────────────────────────────────────────────────

/// `workflow-runner approve` must reach a running daemon, clear the approval
/// gate, and cause the next task to be dispatched with template vars expanded.
#[test]
fn approve_cli_advances_past_awaiting_approval() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_STANDARD);
    let cb_port = pick_free_port();
    let proc = proj.start_run("feature", cb_port, &webhook.url());

    webhook.wait_for_n(1, TIMEOUT); // design dispatched
    proc.complete("design"); // approval: true → awaiting_approval
    std::thread::sleep(Duration::from_millis(100));

    let out = proj.approve_cli(cb_port);
    assert_eq!(out["ok"], true);
    assert_eq!(out["action"], "approve");

    let dispatched = webhook.wait_for_n(2, TIMEOUT);
    assert_eq!(
        dispatched[1]["task_id"].as_str(),
        Some("implement"),
        "implement must be dispatched after `approve` clears the gate"
    );
    let prompt = dispatched[1]["prompt"]
        .as_str()
        .expect("prompt must be present");
    assert!(
        prompt.contains("make test"),
        "{{{{vars.test}}}} must be expanded: got '{prompt}'"
    );
    assert!(
        prompt.contains("make lint"),
        "{{{{vars.lint}}}} must be expanded: got '{prompt}'"
    );
}

// ── 2-2 ───────────────────────────────────────────────────────────────────────

/// `workflow-runner reject <task_id> --reason <text>` must reach the daemon
/// and cause the rejected task to be re-dispatched.
#[test]
fn reject_cli_with_reason_redispatches_task() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_STANDARD);
    let cb_port = pick_free_port();
    let proc = proj.start_run("feature", cb_port, &webhook.url());

    webhook.wait_for_n(1, TIMEOUT);
    proc.complete("design");
    std::thread::sleep(Duration::from_millis(100));

    let out = proj.reject_cli_with_reason("design", cb_port, "Design scope is too broad.");
    assert_eq!(out["ok"], true);
    assert_eq!(out["action"], "reject");
    assert_eq!(out["task_id"], "design");

    let dispatched = webhook.wait_for_n(2, TIMEOUT);
    let ids: Vec<&str> = dispatched
        .iter()
        .map(|d| d["task_id"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        ids.iter().filter(|&&id| id == "design").count(),
        2,
        "design must be dispatched twice (initial + after reject)"
    );
}

// ── 2-3 ───────────────────────────────────────────────────────────────────────

/// `workflow-runner reject` sent while the workflow is NOT awaiting approval
/// must still exit 0 (the daemon was reachable) but must not trigger any
/// redispatch — the daemon silently ignores it server-side. This is the
/// tradeoff of a fire-and-forget HTTP client: the CLI can only confirm
/// delivery, not that the action actually applied.
#[test]
fn reject_cli_outside_approval_state_is_noop() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_STANDARD);
    let cb_port = pick_free_port();
    let _proc = proj.start_run("feature", cb_port, &webhook.url());

    webhook.wait_for_n(1, TIMEOUT); // design dispatched, workflow still "active"

    let out = proj.reject_cli("design", cb_port);
    assert_eq!(
        out["ok"], true,
        "the CLI only confirms the daemon was reachable, not that the reject applied"
    );

    // No new dispatch should occur since the workflow was never awaiting approval.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        webhook.count(),
        1,
        "reject outside awaiting_approval must not cause a redispatch"
    );
}

// ── 2-4 ───────────────────────────────────────────────────────────────────────

/// When the *last* task in a workflow has `approval: true`, `approve` must
/// complete the workflow (the daemon process exits 0).
#[test]
fn approve_cli_on_final_task_completes_workflow() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_SIGN_OFF);
    let cb_port = pick_free_port();
    let mut proc = proj.start_run("sign-off-flow", cb_port, &webhook.url());

    webhook.wait_for_n(1, TIMEOUT); // work dispatched
    proc.complete("work");

    webhook.wait_for_n(2, TIMEOUT); // sign-off dispatched
    proc.complete("sign-off"); // approval: true, last task → awaiting_approval
    std::thread::sleep(Duration::from_millis(100));

    proj.approve_cli(cb_port);

    let status = proc.wait_exit(TIMEOUT);
    assert!(
        status.success(),
        "workflow must complete after the final approval is granted"
    );
}
