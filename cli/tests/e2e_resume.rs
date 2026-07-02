//! E2E tests for Scenario 7: pause / resume (agent-initiated interruption).
//!
//! When a Claude Code worker cannot proceed without user input, it POSTs
//! `/pause/:task_id` to the `run` daemon, which marks the workflow `paused`
//! and stops dispatching further tasks. `workflow-runner resume` (a thin
//! HTTP client, mirroring `approve`/`reject`) POSTs `/resume`, which
//! re-dispatches whatever task was InProgress when the pause happened.
//!
//! Note: this re-dispatch previously silently no-op'd because the paused
//! task_id was never cleared from the daemon's `dispatched` bookkeeping set
//! (dispatch_tasks skips anything already in that set) — the pause/resume
//! path had no test coverage until this file. That bug is fixed in
//! `cmd/run.rs`'s `RunEvent::Resume` handler; these tests guard it.
//!
//! Covers:
//!   7-1  /pause stops further automatic activity; /resume re-dispatches the task
//!   7-2  `workflow-runner resume` CLI reaches the daemon and re-dispatches
//!   7-3  `workflow-runner resume` outside a paused state is a silent no-op

mod helpers;
use helpers::{pick_free_port, MockWebhook, TempProject, CONFIG_MINIMAL};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);

// ── 7-1 ───────────────────────────────────────────────────────────────────────

/// POST /pause must stop dispatch; POST /resume must re-dispatch the same
/// InProgress task and allow the workflow to complete normally afterward.
#[test]
fn pause_then_resume_redispatches_task() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_MINIMAL);
    let cb_port = pick_free_port();
    let mut proc = proj.start_run("simple", cb_port, &webhook.url());

    webhook.wait_for_n(1, TIMEOUT); // only-task dispatched
    proc.pause_with_reason("only-task", "need clarification from the user");

    // No further dispatch happens on its own while paused.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        webhook.count(),
        1,
        "paused workflow must not dispatch anything new"
    );

    proc.resume();

    let dispatched = webhook.wait_for_n(2, TIMEOUT);
    assert_eq!(
        dispatched[1]["task_id"].as_str(),
        Some("only-task"),
        "the paused task must be re-dispatched after /resume"
    );

    // The workflow can still complete normally afterward.
    proc.complete("only-task");
    let status = proc.wait_exit(TIMEOUT);
    assert!(status.success(), "workflow must complete after resuming");
}

// ── 7-2 ───────────────────────────────────────────────────────────────────────

/// `workflow-runner resume --callback-port <port>` must reach the daemon and
/// produce the same re-dispatch as a raw POST /resume.
#[test]
fn resume_cli_redispatches_paused_task() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_MINIMAL);
    let cb_port = pick_free_port();
    let proc = proj.start_run("simple", cb_port, &webhook.url());

    webhook.wait_for_n(1, TIMEOUT);
    proc.pause("only-task");
    std::thread::sleep(Duration::from_millis(100));

    let out = proj.resume_cli(cb_port);
    assert_eq!(out["ok"], true);
    assert_eq!(out["action"], "resume");

    let dispatched = webhook.wait_for_n(2, TIMEOUT);
    assert_eq!(dispatched[1]["task_id"].as_str(), Some("only-task"));
}

// ── 7-3 ───────────────────────────────────────────────────────────────────────

/// `workflow-runner resume` sent while the workflow is NOT paused must still
/// exit 0 (the daemon was reachable) but must not trigger any redispatch.
#[test]
fn resume_cli_outside_paused_state_is_noop() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_MINIMAL);
    let cb_port = pick_free_port();
    let _proc = proj.start_run("simple", cb_port, &webhook.url());

    webhook.wait_for_n(1, TIMEOUT); // workflow is "active", never paused

    let out = proj.resume_cli(cb_port);
    assert_eq!(
        out["ok"], true,
        "the CLI only confirms the daemon was reachable, not that resume applied"
    );

    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        webhook.count(),
        1,
        "resume outside a paused workflow must not cause a redispatch"
    );
}
