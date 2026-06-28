//! E2E tests for Scenario 7: resume (interrupted workflow recovery).
//!
//! Covers:
//!   7-1  resume after partial progress returns the remaining actionable tasks
//!   7-2  resume with no active workflow returns an error
//!   7-3  resume in awaiting_approval state returns the next executable tasks
//!        (resume bypasses the approval gate by design — use `next` for approval)

mod helpers;
use helpers::{TempProject, CONFIG_STANDARD};

// ── 7-1 ───────────────────────────────────────────────────────────────────────

/// After completing some tasks in a session, `resume` (as called at the start
/// of a new session) must return exactly the tasks that can currently run.
#[test]
fn resume_returns_actionable_tasks_after_partial_progress() {
    let proj = TempProject::new(CONFIG_STANDARD);
    proj.start("bug-fix");

    // Complete `reproduce` — now only `identify` is independently runnable.
    // `implement` still needs both `reproduce` (done) and `identify` (pending).
    proj.complete("reproduce");

    // Simulate a new session: `resume` re-derives state from the DB.
    let out = proj.resume();

    assert_eq!(out["status"], "in_progress");
    assert_eq!(out["workflow"], "bug-fix");

    let tasks = out["tasks"].as_array().expect("tasks must be an array");
    let ids: Vec<&str> = tasks
        .iter()
        .map(|t| t["task_id"].as_str().unwrap())
        .collect();

    assert!(
        ids.contains(&"identify"),
        "identify must be actionable after reproduce completes"
    );
    assert!(
        !ids.contains(&"implement"),
        "implement must not appear until both its requires are met"
    );
    assert!(
        !ids.contains(&"reproduce"),
        "completed tasks must not reappear on resume"
    );
}

// ── 7-2 ───────────────────────────────────────────────────────────────────────

/// `resume` with no active workflow must fail with exit code 1.
#[test]
fn resume_without_active_workflow_returns_error() {
    let proj = TempProject::new(CONFIG_STANDARD);
    // No `start` call — nothing in the DB.
    let stderr = proj.assert_err(&["resume"]);
    assert!(
        !stderr.is_empty(),
        "resume must produce an error message when no workflow is active"
    );
}

// ── 7-3 ───────────────────────────────────────────────────────────────────────

/// When a workflow is in `awaiting_approval` state, `resume` returns the next
/// *executable* tasks based on the task-level DB state, bypassing the approval
/// gate.  This is intentional: `resume` is a session-recovery command, not an
/// approval mechanism.  Use `next` (which resets the gate) for approval.
#[test]
fn resume_in_awaiting_approval_returns_executable_tasks() {
    let proj = TempProject::new(CONFIG_STANDARD);
    proj.start("feature");

    // Complete `design` (approval: true) → workflow enters awaiting_approval.
    let complete_out = proj.complete("design");
    assert_eq!(complete_out["next"]["status"], "awaiting_approval");

    // `resume` reflects the DB task states (design=Completed, implement=Pending).
    // Since implement's requires are satisfied, it appears as executable.
    let out = proj.resume();

    assert_eq!(
        out["status"], "in_progress",
        "resume must return in_progress (not awaiting_approval) — \
         it reads task states, not the workflow-level approval gate"
    );
    let tasks = out["tasks"].as_array().expect("tasks must be an array");
    assert_eq!(tasks.len(), 1);
    assert_eq!(
        tasks[0]["task_id"], "implement",
        "implement must appear on resume once design is Completed"
    );
}
