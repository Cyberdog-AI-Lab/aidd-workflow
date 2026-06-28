//! E2E tests for Scenario 1: bug-fix workflow basic lifecycle.
//!
//! Covers:
//!   1-1  start returns the correct initial tasks
//!   1-2  report transitions a task to InProgress
//!   1-3  complete passes when there are no requires
//!   1-4  complete is blocked when requires are not met
//!   1-5  complete passes after requires are met
//!   1-6  full bug-fix flow runs to completion
//!   1-7  status (JSON) reflects the current execution state
//!   1-8  status (table) output contains expected headers and rows
//!   1-9  template vars are expanded in task prompts

mod helpers;
use helpers::{minimal_report, TempProject, CONFIG_STANDARD};

// ── 1-1 ───────────────────────────────────────────────────────────────────────

/// `start bug-fix` must return status="started" and the two initial tasks
/// (reproduce and identify) that have no `requires`.
#[test]
fn start_returns_initial_tasks() {
    let proj = TempProject::new(CONFIG_STANDARD);
    let out = proj.start("bug-fix");

    assert_eq!(out["status"], "started");
    assert_eq!(out["workflow"], "bug-fix");
    assert!(
        out["workflow_id"].as_str().is_some_and(|s| !s.is_empty()),
        "workflow_id must be a non-empty string"
    );

    let tasks = out["tasks"].as_array().expect("tasks must be an array");
    assert_eq!(
        tasks.len(),
        2,
        "reproduce and identify have no requires → both dispatched initially"
    );

    // Tasks are returned in definition order (reproduce first, identify second).
    assert_eq!(tasks[0]["task_id"], "reproduce");
    assert_eq!(tasks[1]["task_id"], "identify");

    // Each task carries a human-readable name.
    assert_eq!(tasks[0]["task"], "Reproduce the bug");
    assert_eq!(tasks[1]["task"], "Identify root cause");

    // implement has requires, so it must NOT appear yet.
    let ids: Vec<&str> = tasks
        .iter()
        .map(|t| t["task_id"].as_str().unwrap())
        .collect();
    assert!(
        !ids.contains(&"implement"),
        "implement must not be dispatched before its requires are met"
    );
}

// ── 1-2 ───────────────────────────────────────────────────────────────────────

/// Sending a `report` for a Pending task must return `ok: true` and the
/// task_id echo.  The task transitions to InProgress in the DB.
#[test]
fn report_transitions_to_in_progress() {
    let proj = TempProject::new(CONFIG_STANDARD);
    proj.start("bug-fix");

    let body = minimal_report();
    let out = proj.report("reproduce", &body);

    assert_eq!(out["ok"], true);
    assert_eq!(out["task_id"], "reproduce");

    // Confirm the DB reflects InProgress via `status`.
    let status = proj.status();
    let tasks = status["tasks"].as_array().expect("tasks must be an array");
    let reproduce = tasks
        .iter()
        .find(|t| t["id"] == "reproduce")
        .expect("reproduce must be in status");
    assert_eq!(
        reproduce["status"], "inprogress",
        "reproduce must be InProgress after report"
    );
}

// ── 1-3 ───────────────────────────────────────────────────────────────────────

/// `complete reproduce` must succeed: `reproduce` has no `requires`.
/// The response must carry the next executable task (identify).
#[test]
fn complete_without_requires_passes() {
    let proj = TempProject::new(CONFIG_STANDARD);
    proj.start("bug-fix");

    let out = proj.complete("reproduce");

    assert_eq!(
        out["allowed"], true,
        "gate must pass for a task with no requires"
    );
    assert!(out["reason"].is_null(), "reason must be null when allowed");

    let next = &out["next"];
    assert_eq!(next["status"], "in_progress");

    let next_tasks = next["tasks"]
        .as_array()
        .expect("next.tasks must be an array");
    let ids: Vec<&str> = next_tasks
        .iter()
        .map(|t| t["task_id"].as_str().unwrap())
        .collect();
    assert!(
        ids.contains(&"identify"),
        "identify must be in next tasks after reproduce completes"
    );
    // implement still blocked — only 1 of its 2 requires is met.
    assert!(
        !ids.contains(&"implement"),
        "implement must not appear until both requires are met"
    );
}

// ── 1-4 ───────────────────────────────────────────────────────────────────────

/// `complete implement` on a fresh workflow must be blocked: its `requires`
/// (reproduce, identify) are not yet Completed.
/// The response has exit code 0, but `allowed: false` with a reason.
#[test]
fn complete_with_unmet_requires_blocked() {
    let proj = TempProject::new(CONFIG_STANDARD);
    proj.start("bug-fix");

    // attempt to complete implement without satisfying its requires
    let out = proj.complete("implement");

    assert_eq!(
        out["allowed"], false,
        "gate must block when requires are not met"
    );
    let reason = out["reason"].as_str().expect("reason must be a string");
    // Gate reports the first unmet dependency (reproduce comes before identify in config).
    assert!(
        reason.contains("reproduce"),
        "reason must mention the unmet dependency: got '{reason}'"
    );
    assert!(out["next"].is_null(), "next must be null when blocked");
}

// ── 1-5 ───────────────────────────────────────────────────────────────────────

/// After completing both `reproduce` and `identify`, `complete implement` must pass.
#[test]
fn complete_after_requires_met_passes() {
    let proj = TempProject::new(CONFIG_STANDARD);
    proj.start("bug-fix");

    proj.complete("reproduce");
    proj.complete("identify");

    let out = proj.complete("implement");
    assert_eq!(
        out["allowed"], true,
        "gate must pass once all requires are Completed"
    );

    let next = &out["next"];
    assert_eq!(next["status"], "in_progress");
    let tasks = next["tasks"]
        .as_array()
        .expect("next.tasks must be an array");
    let ids: Vec<&str> = tasks
        .iter()
        .map(|t| t["task_id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        vec!["complete"],
        "only the final 'complete' task should remain"
    );
}

// ── 1-6 ───────────────────────────────────────────────────────────────────────

/// Running through every task in the correct order must reach `status: "completed"`.
#[test]
fn full_bug_fix_flow_to_completion() {
    let proj = TempProject::new(CONFIG_STANDARD);
    let start_out = proj.start("bug-fix");

    let workflow_id = start_out["workflow_id"]
        .as_str()
        .expect("workflow_id must be a string");

    // Both requires-free tasks can be completed in any order.
    let out = proj.complete("reproduce");
    assert_eq!(out["allowed"], true);

    let out = proj.complete("identify");
    assert_eq!(out["allowed"], true);

    // implement is now unblocked.
    let out = proj.complete("implement");
    assert_eq!(out["allowed"], true);
    assert_eq!(out["next"]["status"], "in_progress");

    // Final task — workflow should complete.
    let out = proj.complete("complete");
    assert_eq!(out["allowed"], true);
    assert_eq!(
        out["next"]["status"], "completed",
        "workflow must reach 'completed' after all tasks are done"
    );
    // workflow_id is echoed back in the final response.
    assert_eq!(out["next"]["workflow_id"], workflow_id);
}

// ── 1-7 ───────────────────────────────────────────────────────────────────────

/// `status --format json` must accurately reflect the current task statuses.
#[test]
fn status_json_reflects_current_state() {
    let proj = TempProject::new(CONFIG_STANDARD);
    proj.start("bug-fix");
    proj.complete("reproduce");

    let status = proj.status(); // defaults to JSON format

    // Top-level fields
    assert_eq!(status["workflow"], "bug-fix");
    assert!(status["started_at"].as_str().is_some_and(|s| !s.is_empty()));

    let tasks = status["tasks"].as_array().expect("tasks must be an array");
    // Tasks appear in config definition order.
    assert_eq!(tasks.len(), 4);

    let find = |id: &str| -> &serde_json::Value {
        tasks
            .iter()
            .find(|t| t["id"] == id)
            .unwrap_or_else(|| panic!("task '{id}' not found in status"))
    };

    assert_eq!(find("reproduce")["status"], "completed");
    assert_eq!(find("identify")["status"], "pending");
    assert_eq!(find("implement")["status"], "pending");
    assert_eq!(find("complete")["status"], "pending");
}

// ── 1-8 ───────────────────────────────────────────────────────────────────────

/// `status --format table` must emit a human-readable ASCII table with the
/// expected column headers and at least one data row.
#[test]
fn status_table_format_includes_header() {
    let proj = TempProject::new(CONFIG_STANDARD);
    proj.start("bug-fix");
    proj.complete("reproduce");

    // Table output is plain text, not JSON — use raw run() to capture it.
    let output = proj.run(&["status", "--format", "table"]);
    assert!(output.status.success(), "status --format table must exit 0");

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("TASK ID"), "table must have a TASK ID column");
    assert!(text.contains("TASK"), "table must have a TASK column");
    assert!(text.contains("STATUS"), "table must have a STATUS column");

    // Task IDs and their statuses must appear in the output.
    assert!(text.contains("reproduce"), "reproduce must appear in table");
    assert!(
        text.contains("completed"),
        "completed status must appear in table"
    );
    assert!(text.contains("identify"), "identify must appear in table");

    // Session metadata block
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
/// when the task is returned by `start` or `complete`.
#[test]
fn template_vars_are_expanded_in_prompt() {
    let proj = TempProject::new(CONFIG_STANDARD);
    proj.start("bug-fix");

    proj.complete("reproduce");
    proj.complete("identify");

    // implement's prompt contains {{vars.test}} → should expand to "make test".
    let out = proj.complete("implement");
    let next_tasks = out["next"]["tasks"]
        .as_array()
        .expect("next.tasks must be an array");

    // 'complete' (the final task) has a static prompt with no vars.
    // Verify implement itself was visible in the previous next response.
    // Re-start and re-complete up to the point where implement is dispatched.
    let proj2 = TempProject::new(CONFIG_STANDARD);
    let _ = proj2.start("bug-fix");
    // reproduce and identify are dispatched first — implement's prompt is not visible yet.
    let _ = next_tasks; // suppress unused warning

    proj2.complete("reproduce");
    proj2.complete("identify");

    // After both requires are met, implement appears via `next`.
    let next_out = proj2.next();
    let tasks = next_out["tasks"]
        .as_array()
        .expect("tasks must be an array");
    let implement_task = tasks
        .iter()
        .find(|t| t["task_id"] == "implement")
        .expect("implement must appear in next tasks");

    let prompt = implement_task["prompt"]
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
