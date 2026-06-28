//! E2E tests for Scenario 3: release workflow — parallel agents.
//!
//! Covers:
//!   3-1  quality-check dispatches the agents list (not a prompt)
//!   3-2  completing a sub-agent does not auto-complete the parent
//!   3-3  complete on the parent is gated until all agents finish
//!   3-4  parent passes the gate once every agent is Completed
//!   3-5  reporting for a sub-agent transitions the parent to InProgress

mod helpers;
use helpers::{minimal_report, TempProject, CONFIG_STANDARD};

// ── helpers ───────────────────────────────────────────────────────────────────

/// Advance the release workflow from `start` to the point where `quality-check`
/// has been dispatched (implement is Completed, quality-check is Pending).
///
/// Flow:
///   start release
///   → complete design (approval: true)
///   → next (approve design)
///   → complete implement
fn advance_to_quality_check(proj: &TempProject) {
    proj.start("release");
    proj.complete("design"); // approval: true → awaiting_approval
    proj.next(); // approve → implement is dispatched
    proj.complete("implement"); // quality-check becomes executable
}

// ── 3-1 ───────────────────────────────────────────────────────────────────────

/// When `implement` completes, `quality-check` (an agents task) must be returned
/// with an `agents` list and a null `prompt`.  No individual sub-task IDs are
/// exposed at this level — the SKILL layer spawns agents by name.
#[test]
fn quality_check_task_returns_agents_list() {
    let proj = TempProject::new(CONFIG_STANDARD);

    proj.start("release");
    proj.complete("design");
    proj.next(); // approve

    let out = proj.complete("implement");
    assert_eq!(out["allowed"], true);

    let tasks = out["next"]["tasks"]
        .as_array()
        .expect("next.tasks must be an array");
    assert_eq!(
        tasks.len(),
        1,
        "only quality-check should be dispatched after implement"
    );

    let qc = &tasks[0];
    assert_eq!(qc["task_id"], "quality-check");
    assert_eq!(qc["task"], "Quality check");
    assert!(
        qc["prompt"].is_null(),
        "agents-only tasks must have a null prompt"
    );

    // Both agent names must be present (order preserved from config).
    let agents = qc["agents"].as_array().expect("agents must be an array");
    assert_eq!(agents.len(), 2);
    assert_eq!(agents[0], "run-test");
    assert_eq!(agents[1], "run-lint");
}

// ── 3-2 ───────────────────────────────────────────────────────────────────────

/// Completing a sub-agent (`quality-check/run-test`) must:
///   - allow the transition (gate always passes for registered sub-agents)
///   - NOT auto-complete the parent task
///   - transition the parent to InProgress via sync_agents_parent
///   - leave the workflow blocked (run-lint is still running)
#[test]
fn sub_agent_complete_does_not_complete_parent() {
    let proj = TempProject::new(CONFIG_STANDARD);
    advance_to_quality_check(&proj);

    let out = proj.complete("quality-check/run-test");

    assert_eq!(
        out["allowed"], true,
        "sub-agent completion must always be allowed"
    );
    assert_eq!(
        out["task_id"], "quality-check/run-test",
        "task_id must reflect the sub-agent"
    );

    // Workflow is blocked: quality-check is InProgress (agent task) so it is
    // not re-dispatched, and `complete` is blocked by quality-check.
    assert_eq!(
        out["next"]["status"], "blocked",
        "workflow must be blocked while run-lint is still pending"
    );
    assert_eq!(
        out["next"]["tasks"].as_array().map(|a| a.len()),
        Some(0),
        "no tasks should be dispatched while waiting for the remaining agent"
    );

    // Verify DB state via `status`.
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
}

// ── 3-3 ───────────────────────────────────────────────────────────────────────

/// Attempting to complete the parent task while at least one agent is still
/// pending must be blocked by the gate, with a reason naming the unfinished agent.
#[test]
fn parent_gated_until_all_agents_complete() {
    let proj = TempProject::new(CONFIG_STANDARD);
    advance_to_quality_check(&proj);

    // Complete only one of the two agents.
    proj.complete("quality-check/run-test");

    // Parent completion must be rejected: run-lint is still pending.
    let out = proj.complete("quality-check");

    assert_eq!(
        out["allowed"], false,
        "parent gate must block until every agent is Completed"
    );
    let reason = out["reason"].as_str().expect("reason must be a string");
    assert!(
        reason.contains("run-lint"),
        "reason must name the unfinished agent: got '{reason}'"
    );
    assert!(out["next"].is_null());
}

// ── 3-4 ───────────────────────────────────────────────────────────────────────

/// Once every agent is Completed, `complete quality-check` must pass the gate
/// and dispatch the next task (`complete`, which has `approval: true`).
#[test]
fn parent_passes_gate_when_all_agents_complete() {
    let proj = TempProject::new(CONFIG_STANDARD);
    advance_to_quality_check(&proj);

    // Complete both agents.
    let out = proj.complete("quality-check/run-test");
    assert_eq!(out["allowed"], true);
    let out = proj.complete("quality-check/run-lint");
    assert_eq!(out["allowed"], true);

    // Now the parent gate must pass.
    let out = proj.complete("quality-check");
    assert_eq!(
        out["allowed"], true,
        "parent must pass the gate once all agents are Completed"
    );

    // `complete` (the next task) is dispatched.
    let next = &out["next"];
    assert_eq!(next["status"], "in_progress");
    let next_tasks = next["tasks"]
        .as_array()
        .expect("next.tasks must be an array");
    assert_eq!(next_tasks.len(), 1);
    assert_eq!(next_tasks[0]["task_id"], "complete");
    // The final `complete` task has approval: true.
    assert_eq!(next_tasks[0]["approval"], true);
}

// ── 3-5 ───────────────────────────────────────────────────────────────────────

/// When `report` is called for a sub-agent, the parent task must transition
/// from Pending to InProgress (`sync_agents_parent`).
#[test]
fn report_to_sub_agent_transitions_parent_to_in_progress() {
    let proj = TempProject::new(CONFIG_STANDARD);
    advance_to_quality_check(&proj);

    // Confirm the parent starts as Pending.
    let status_before = proj.status();
    let tasks_before = status_before["tasks"].as_array().unwrap();
    let qc_before = tasks_before
        .iter()
        .find(|t| t["id"] == "quality-check")
        .unwrap();
    assert_eq!(qc_before["status"], "pending");

    // Report for one sub-agent.
    let body = minimal_report();
    let out = proj.report("quality-check/run-test", &body);
    assert_eq!(out["ok"], true);
    assert_eq!(out["task_id"], "quality-check/run-test");

    // Both the sub-agent and the parent should now be InProgress.
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
        "parent must transition to InProgress when any sub-agent reports"
    );
    assert_eq!(
        find("quality-check/run-test")["status"],
        "inprogress",
        "the reporting sub-agent must be InProgress"
    );
    assert_eq!(
        find("quality-check/run-lint")["status"],
        "pending",
        "the other sub-agent must remain Pending"
    );
}
