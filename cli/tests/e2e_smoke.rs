//! Smoke test: verifies the helpers compile and the binary is reachable.
//! This file can be removed once real E2E tests are added.

mod helpers;

#[test]
fn binary_is_reachable() {
    let proj = helpers::TempProject::new(helpers::CONFIG_MINIMAL);
    let out = proj.start("simple");
    assert_eq!(out["status"], "started");
    let tasks = out["tasks"].as_array().expect("tasks should be an array");
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["task_id"], "only-task");
}
