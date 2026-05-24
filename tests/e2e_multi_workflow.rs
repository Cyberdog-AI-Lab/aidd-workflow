//! E2E tests for Scenario 5: multiple concurrent workflows.
//!
//! Covers:
//!   5-1  `next` without --workflow-id fails when two workflows are active
//!   5-2  --workflow-id routes each command to the correct workflow

mod helpers;
use helpers::{TempProject, CONFIG_STANDARD};

// ── 5-1 ───────────────────────────────────────────────────────────────────────

/// Starting two workflows from the same project directory leaves two active
/// entries in the DB.  Calling `next` without --workflow-id must fail with an
/// informative error asking the user to disambiguate.
#[test]
fn two_active_workflows_next_without_id_fails() {
    let proj = TempProject::new(CONFIG_STANDARD);

    // Start two different workflows in the same project.
    proj.start("bug-fix");
    proj.start("feature");

    // `next` without --workflow-id cannot know which workflow to advance.
    let stderr = proj.assert_err(&["next"]);
    assert!(
        stderr.contains("--workflow-id"),
        "error must instruct the user to pass --workflow-id: got '{stderr}'"
    );
}

// ── 5-2 ───────────────────────────────────────────────────────────────────────

/// --workflow-id must route each command to the correct independent workflow.
/// Changes to one workflow must not affect the other.
#[test]
fn workflow_id_routes_to_correct_workflow() {
    let proj = TempProject::new(CONFIG_STANDARD);

    // Start both workflows and capture their IDs.
    let out1 = proj.start("bug-fix");
    let wf_id1 = out1["workflow_id"]
        .as_str()
        .expect("workflow_id must be a string")
        .to_string();

    let out2 = proj.start("feature");
    let wf_id2 = out2["workflow_id"]
        .as_str()
        .expect("workflow_id must be a string")
        .to_string();

    assert_ne!(
        wf_id1, wf_id2,
        "each start must generate a unique workflow_id"
    );

    // `next` for workflow 1 must return bug-fix tasks.
    let next1 = proj.next_with_id(&wf_id1);
    assert_eq!(next1["workflow"], "bug-fix");
    let tasks1 = next1["tasks"].as_array().expect("tasks must be an array");
    let ids1: Vec<&str> = tasks1
        .iter()
        .map(|t| t["task_id"].as_str().unwrap())
        .collect();
    assert!(
        ids1.contains(&"reproduce"),
        "bug-fix must return reproduce as a next task"
    );
    assert!(
        ids1.contains(&"identify"),
        "bug-fix must return identify as a next task"
    );

    // `next` for workflow 2 must return feature tasks.
    let next2 = proj.next_with_id(&wf_id2);
    assert_eq!(next2["workflow"], "feature");
    let tasks2 = next2["tasks"].as_array().expect("tasks must be an array");
    assert_eq!(tasks2.len(), 1);
    assert_eq!(tasks2[0]["task_id"], "design");

    // Advancing workflow 1 must not affect workflow 2's state.
    proj.complete_with_id(&wf_id1, "reproduce");

    // bug-fix now shows identify (reproduce done), feature is still unchanged.
    let after_complete = proj.next_with_id(&wf_id1);
    let ids_after: Vec<&str> = after_complete["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["task_id"].as_str().unwrap())
        .collect();
    assert!(
        ids_after.contains(&"identify"),
        "identify must remain after reproduce is done"
    );
    assert!(
        !ids_after.contains(&"reproduce"),
        "reproduce must not reappear after completion"
    );

    let feature_unchanged = proj.next_with_id(&wf_id2);
    assert_eq!(
        feature_unchanged["tasks"][0]["task_id"], "design",
        "feature workflow must be unaffected"
    );
}
