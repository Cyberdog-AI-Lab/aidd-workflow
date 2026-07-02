//! E2E tests for Scenario 3: release workflow — parallel agents.
//!
//! Task progress is driven through a `workflow-runner run` daemon (autonomous
//! execution mode) via HTTP callbacks, mirroring how a real Claude Code
//! worker would report sub-agent completions.
//!
//! Covers:
//!   3-1  quality-check dispatches the agents list (not a prompt)
//!   3-2  completing a sub-agent does not auto-complete the parent
//!   3-3  complete on the parent is gated until all agents finish
//!   3-4  parent passes the gate once every agent is Completed
//!   3-5  reporting for a sub-agent transitions the parent to InProgress

mod helpers;
use helpers::{
    pick_free_port, wait_until, MockWebhook, RunningProcess, TempProject, CONFIG_STANDARD,
};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);

// ── helpers ───────────────────────────────────────────────────────────────────

/// Start the release workflow via the `run` daemon and advance it to the
/// point where `quality-check` (the agents task) has just been dispatched.
///
/// Flow: run release → design dispatched → complete design (approval) →
/// approve → implement dispatched → complete implement → quality-check dispatched.
fn advance_to_quality_check(
    proj: &TempProject,
    webhook: &MockWebhook,
    cb_port: u16,
) -> RunningProcess {
    let proc = proj.start_run("release", cb_port, &webhook.url());

    webhook.wait_for_n(1, TIMEOUT); // design dispatched
    proc.complete("design"); // approval: true → awaiting_approval
    std::thread::sleep(Duration::from_millis(100));
    proc.approve(); // implement dispatched

    webhook.wait_for_n(2, TIMEOUT);
    proc.complete("implement"); // quality-check becomes executable

    webhook.wait_for_n(3, TIMEOUT); // quality-check dispatched
    proc
}

// ── 3-1 ───────────────────────────────────────────────────────────────────────

/// When `implement` completes, `quality-check` (an agents task) must be
/// dispatched with an `agents` list and a null `prompt`. No individual
/// sub-task IDs are exposed at this level — the worker spawns agents by name.
#[test]
fn quality_check_task_returns_agents_list() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_STANDARD);
    let cb_port = pick_free_port();

    let _proc = advance_to_quality_check(&proj, &webhook, cb_port);

    let dispatched = webhook.received();
    assert_eq!(
        dispatched.len(),
        3,
        "only design, implement, and quality-check should be dispatched"
    );

    let qc = &dispatched[2];
    assert_eq!(qc["task_id"], "quality-check");
    assert_eq!(qc["task"], "Quality check");
    assert!(
        qc["prompt"].is_null(),
        "agents-only tasks must have a null prompt"
    );

    let agents = qc["agents"].as_array().expect("agents must be an array");
    assert_eq!(agents.len(), 2);
    assert_eq!(agents[0], "run-test");
    assert_eq!(agents[1], "run-lint");
}

// ── 3-2 ───────────────────────────────────────────────────────────────────────

/// Completing a sub-agent (`quality-check/run-test`) must:
///   - transition the parent to InProgress via sync_agents_parent
///   - NOT auto-complete the parent task
///   - leave run-lint Pending (still running)
#[test]
fn sub_agent_complete_does_not_complete_parent() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_STANDARD);
    let cb_port = pick_free_port();
    let proc = advance_to_quality_check(&proj, &webhook, cb_port);

    proc.complete("quality-check/run-test");

    wait_until(TIMEOUT, || {
        let status = proj.status();
        status["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == "quality-check/run-test" && t["status"] == "completed")
    });

    let status = proj.status();
    let tasks = status["tasks"].as_array().expect("tasks must be an array");

    let find = |id: &str| -> &serde_json::Value {
        tasks
            .iter()
            .find(|t| t["id"] == id)
            .unwrap_or_else(|| panic!("task '{id}' not found in status"))
    };

    // Parent transitions to InProgress when any agent becomes non-Pending.
    assert_eq!(find("quality-check")["status"], "inprogress");
    // Sub-tasks are visible individually in the status output.
    assert_eq!(find("quality-check/run-test")["status"], "completed");
    assert_eq!(find("quality-check/run-lint")["status"], "pending");

    // No new dispatch (quality-check itself is InProgress, not re-dispatched).
    assert_eq!(
        webhook.count(),
        3,
        "no further dispatch while run-lint is still pending"
    );
}

// ── 3-3 ───────────────────────────────────────────────────────────────────────

/// Attempting to complete the parent task while at least one agent is still
/// pending must be blocked by the gate — the daemon logs it and does not
/// dispatch anything further.
#[test]
fn parent_gated_until_all_agents_complete() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_STANDARD);
    let cb_port = pick_free_port();
    let proc = advance_to_quality_check(&proj, &webhook, cb_port);

    // Complete only one of the two agents.
    proc.complete("quality-check/run-test");
    wait_until(TIMEOUT, || {
        let status = proj.status();
        status["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == "quality-check/run-test" && t["status"] == "completed")
    });

    // Attempting to complete the parent must be gated out server-side.
    proc.complete("quality-check");
    std::thread::sleep(Duration::from_millis(300));

    let status = proj.status();
    let tasks = status["tasks"].as_array().unwrap();
    let qc = tasks.iter().find(|t| t["id"] == "quality-check").unwrap();
    assert_eq!(
        qc["status"], "inprogress",
        "parent must remain InProgress; the gate must reject completion while run-lint is pending"
    );
    assert_eq!(
        webhook.count(),
        3,
        "no further dispatch while the parent gate is blocked"
    );
}

// ── 3-4 ───────────────────────────────────────────────────────────────────────

/// Once every agent is Completed, `complete quality-check` must pass the gate
/// and dispatch the next task (`complete`, which has `approval: true`).
#[test]
fn parent_passes_gate_when_all_agents_complete() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_STANDARD);
    let cb_port = pick_free_port();
    let proc = advance_to_quality_check(&proj, &webhook, cb_port);

    // Complete both agents.
    proc.complete("quality-check/run-test");
    proc.complete("quality-check/run-lint");
    wait_until(TIMEOUT, || {
        let status = proj.status();
        let tasks = status["tasks"].as_array().unwrap();
        tasks
            .iter()
            .find(|t| t["id"] == "quality-check/run-lint")
            .is_some_and(|t| t["status"] == "completed")
    });

    // Now the parent gate must pass and `complete` (approval: true) is dispatched.
    proc.complete("quality-check");
    let dispatched = webhook.wait_for_n(4, TIMEOUT);
    assert_eq!(
        dispatched[3]["task_id"].as_str(),
        Some("complete"),
        "the final 'complete' task must be dispatched once the parent gate passes"
    );
}

// ── 3-5 ───────────────────────────────────────────────────────────────────────

/// `/report` on a sub-agent only appends an action report — it must not
/// change any task's status (neither the reporting sub-agent nor its
/// sibling). Unlike the old synchronous `report` command, the daemon's
/// dispatcher already marks an agents-parent task InProgress the moment it
/// is dispatched (see 3-1), so there is no separate "report flips the parent
/// to InProgress" transition to observe here — the parent is InProgress
/// before any sub-agent ever reports.
#[test]
fn report_on_sub_agent_does_not_change_task_statuses() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_STANDARD);
    let cb_port = pick_free_port();
    let proc = advance_to_quality_check(&proj, &webhook, cb_port);

    let status_before = proj.status();
    let tasks_before = status_before["tasks"].as_array().unwrap();
    let qc_before = tasks_before
        .iter()
        .find(|t| t["id"] == "quality-check")
        .unwrap();
    assert_eq!(
        qc_before["status"], "inprogress",
        "the parent is already InProgress as soon as it is dispatched"
    );

    // Report for one sub-agent. No further dispatch follows a report, so
    // give the daemon's async handler a moment to apply it before asserting.
    proc.report("quality-check/run-test", "in progress");
    std::thread::sleep(Duration::from_millis(300));

    let status_after = proj.status();
    let tasks_after = status_after["tasks"].as_array().unwrap();

    let find = |id: &str| -> &serde_json::Value {
        tasks_after
            .iter()
            .find(|t| t["id"] == id)
            .unwrap_or_else(|| panic!("task '{id}' not found in status"))
    };

    assert_eq!(
        find("quality-check")["status"],
        "inprogress",
        "report must not change the parent's status"
    );
    assert_eq!(
        find("quality-check/run-test")["status"],
        "pending",
        "report must not change the reporting sub-agent's own status"
    );
    assert_eq!(
        find("quality-check/run-lint")["status"],
        "pending",
        "the other sub-agent must remain unaffected"
    );
}
