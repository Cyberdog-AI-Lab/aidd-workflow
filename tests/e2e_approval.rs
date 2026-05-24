//! E2E tests for Scenario 2: approval / reject flow.
//!
//! Covers:
//!   2-1  complete on an approval task transitions to awaiting_approval
//!   2-2  next acts as approval and returns the next tasks
//!   2-3  reject resets the task to InProgress and clears the approval gate
//!   2-4  reject with --reason records the reason in the output
//!   2-5  reject outside of awaiting_approval state returns an error
//!   2-6  next after the final approval task completes the workflow

mod helpers;
use helpers::{TempProject, CONFIG_STANDARD};

/// Config with a two-task workflow whose last task requires sign-off (approval: true).
/// Used by 2-6 to test that `next` after the final approval exits the workflow cleanly.
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

// ── helpers ───────────────────────────────────────────────────────────────────

/// Bring the feature workflow to `awaiting_approval` by completing `design`.
/// Returns the workflow_id of the started workflow.
fn start_feature_and_complete_design(proj: &TempProject) -> String {
    let start = proj.start("feature");
    let workflow_id = start["workflow_id"]
        .as_str()
        .expect("workflow_id must be a string")
        .to_string();
    proj.complete("design");
    workflow_id
}

// ── 2-1 ───────────────────────────────────────────────────────────────────────

/// Completing a task that has `approval: true` must return
/// `status: "awaiting_approval"` with an empty task list.
/// The workflow is paused until the developer calls `next` (approve) or
/// `reject` (re-do).
#[test]
fn complete_approval_task_sets_awaiting_approval() {
    let proj = TempProject::new(CONFIG_STANDARD);
    proj.start("feature");

    // `design` has `approval: true` in CONFIG_STANDARD.
    let out = proj.complete("design");

    assert_eq!(out["allowed"], true);
    assert!(out["reason"].is_null());

    let next = &out["next"];
    assert_eq!(
        next["status"], "awaiting_approval",
        "completing an approval task must pause the workflow"
    );
    assert_eq!(
        next["tasks"].as_array().map(|a| a.len()),
        Some(0),
        "no tasks are dispatched while the workflow awaits approval"
    );
}

// ── 2-2 ───────────────────────────────────────────────────────────────────────

/// Calling `next` while in `awaiting_approval` state approves the reviewed
/// task and returns the next set of executable tasks.
#[test]
fn next_approves_and_proceeds_to_implement() {
    let proj = TempProject::new(CONFIG_STANDARD);
    start_feature_and_complete_design(&proj);

    // `next` acts as the approval.
    let out = proj.next();

    assert_eq!(
        out["status"], "in_progress",
        "workflow must resume after approval"
    );
    let tasks = out["tasks"].as_array().expect("tasks must be an array");
    assert_eq!(
        tasks.len(),
        1,
        "exactly one task must be dispatched after approval"
    );
    assert_eq!(tasks[0]["task_id"], "implement");

    // The implement task's prompt must have its vars expanded.
    let prompt = tasks[0]["prompt"]
        .as_str()
        .expect("implement must have a prompt");
    assert!(
        prompt.contains("make test"),
        "{{{{vars.test}}}} must be expanded in the prompt"
    );
    assert!(
        prompt.contains("make lint"),
        "{{{{vars.lint}}}} must be expanded in the prompt"
    );
}

// ── 2-3 ───────────────────────────────────────────────────────────────────────

/// `reject <task_id>` while in `awaiting_approval` must:
///   - return the task definition for re-dispatch
///   - reset the task's DB status to InProgress
///   - clear the approval gate (workflow back to active)
#[test]
fn reject_resets_task_to_in_progress() {
    let proj = TempProject::new(CONFIG_STANDARD);
    start_feature_and_complete_design(&proj);

    let out = proj.reject("design");

    // Response structure
    assert_eq!(out["task_id"], "design");
    assert!(
        out["reason"].is_null(),
        "reason must be null when no --reason is given"
    );

    // The embedded task output is provided for re-dispatch.
    let task = &out["task"];
    assert_eq!(task["task_id"], "design");
    assert_eq!(task["task"], "Write design doc");
    assert_eq!(
        task["approval"], true,
        "approval flag must be preserved in the re-dispatch output"
    );

    // DB must reflect InProgress (not Completed) for `design`.
    let status = proj.status();
    let tasks = status["tasks"].as_array().expect("tasks must be an array");
    let design = tasks
        .iter()
        .find(|t| t["id"] == "design")
        .expect("design must appear in status");
    assert_eq!(
        design["status"], "inprogress",
        "design must be InProgress after reject"
    );

    // Workflow must be active again, so `next` returns design for re-work.
    let next_out = proj.next();
    assert_eq!(next_out["status"], "in_progress");
    let next_tasks = next_out["tasks"]
        .as_array()
        .expect("tasks must be an array");
    let ids: Vec<&str> = next_tasks
        .iter()
        .map(|t| t["task_id"].as_str().unwrap())
        .collect();
    assert!(
        ids.contains(&"design"),
        "design must be re-dispatched after reject"
    );
}

// ── 2-4 ───────────────────────────────────────────────────────────────────────

/// `reject <task_id> --reason <text>` must include the developer's feedback
/// in the response `reason` field.
#[test]
fn reject_with_reason_is_recorded_in_output() {
    let proj = TempProject::new(CONFIG_STANDARD);
    start_feature_and_complete_design(&proj);

    let reason_text = "Design scope is too broad; split into two separate documents.";
    let out = proj.reject_with_reason("design", reason_text);

    assert_eq!(out["task_id"], "design");
    assert_eq!(
        out["reason"].as_str(),
        Some(reason_text),
        "the --reason text must appear verbatim in the response"
    );

    // Task definition is still returned for re-dispatch.
    assert_eq!(out["task"]["task_id"], "design");
}

// ── 2-5 ───────────────────────────────────────────────────────────────────────

/// `reject` when the workflow is NOT in `awaiting_approval` state must fail
/// with exit code 1 and an informative error message.
#[test]
fn reject_outside_approval_state_returns_error() {
    let proj = TempProject::new(CONFIG_STANDARD);
    // Start the workflow but do NOT call `complete design` — still in active state.
    proj.start("feature");

    let stderr = proj.assert_err(&["reject", "design"]);

    assert!(
        stderr.contains("not awaiting approval"),
        "error must explain that the workflow is not in awaiting_approval state: got '{stderr}'"
    );
}

// ── 2-6 ───────────────────────────────────────────────────────────────────────

/// When the *last* task in a workflow has `approval: true`, calling `next`
/// after the approval must complete the workflow (status: "completed").
///
/// Flow:
///   start → complete work → complete sign-off (approval) → next (approve) → completed
#[test]
fn next_after_final_approval_completes_workflow() {
    let proj = TempProject::new(CONFIG_SIGN_OFF);

    proj.start("sign-off-flow");
    proj.complete("work");

    // `sign-off` has `approval: true` and is the last task.
    let out = proj.complete("sign-off");
    assert_eq!(out["allowed"], true);
    assert_eq!(
        out["next"]["status"], "awaiting_approval",
        "workflow must pause for sign-off approval"
    );

    // Approving with `next` should find all tasks Completed → workflow ends.
    let final_out = proj.next();
    assert_eq!(
        final_out["status"], "completed",
        "workflow must be completed after the final approval is granted"
    );
    assert_eq!(
        final_out["tasks"].as_array().map(|a| a.len()),
        Some(0),
        "no more tasks after workflow completion"
    );
}
