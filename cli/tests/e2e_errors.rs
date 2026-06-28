//! E2E tests for Scenario 8: error cases and edge-case behaviours.
//!
//! Covers:
//!   8-1  start with an unknown workflow slug returns an error
//!   8-2  complete on an unknown task returns allowed:false (gate, not exit 1)
//!   8-3  next without an active workflow returns an error
//!   8-4  report with an unknown task_id returns an error
//!   8-5  undefined {{vars.key}} placeholders are preserved verbatim in prompts
//!   8-6  imports: an external workflow file is resolved and merged
//!   8-7  imports: a circular reference is detected and reported
//!   8-8  imports: diamond imports (A→B→shared, A→C→shared) are allowed

mod helpers;
use helpers::{minimal_report, TempProject, CONFIG_STANDARD};

// ── 8-1 ───────────────────────────────────────────────────────────────────────

/// `start` with a workflow slug that does not exist in config.yml must exit 1
/// with an error message that includes the unknown slug.
#[test]
fn start_unknown_workflow_exits_with_error() {
    let proj = TempProject::new(CONFIG_STANDARD);
    let stderr = proj.assert_err(&["start", "does-not-exist"]);
    assert!(
        stderr.contains("does-not-exist"),
        "error must mention the unknown slug: got '{stderr}'"
    );
}

// ── 8-2 ───────────────────────────────────────────────────────────────────────

/// `complete` on a task ID that does not exist in the workflow config must
/// return exit 0 with `allowed: false` — the gate check handles this case
/// gracefully rather than crashing.
#[test]
fn complete_unknown_task_returns_allowed_false() {
    let proj = TempProject::new(CONFIG_STANDARD);
    proj.start("bug-fix");

    let out = proj.complete("ghost-task");

    assert_eq!(
        out["allowed"], false,
        "unknown task must be gated out (allowed:false), not cause an error exit"
    );
    let reason = out["reason"].as_str().expect("reason must be present");
    assert!(
        reason.contains("ghost-task"),
        "reason must mention the unknown task id: got '{reason}'"
    );
}

// ── 8-3 ───────────────────────────────────────────────────────────────────────

/// `next` when no workflow is active (nothing has been started) must exit 1.
#[test]
fn next_without_active_workflow_exits_with_error() {
    let proj = TempProject::new(CONFIG_STANDARD);
    let stderr = proj.assert_err(&["next"]);
    assert!(
        !stderr.is_empty(),
        "next must produce an error message when no workflow is active"
    );
}

// ── 8-4 ───────────────────────────────────────────────────────────────────────

/// `report` with a task_id that was never registered in the workflow state
/// must exit 1 — the command validates task IDs against the initialised state.
#[test]
fn report_unknown_task_id_exits_with_error() {
    let proj = TempProject::new(CONFIG_STANDARD);
    proj.start("bug-fix");

    let body = minimal_report("phantom-task");
    let json = serde_json::to_string(&body).unwrap();
    let stderr = proj.assert_err_with_stdin(&["report"], &json);
    assert!(
        stderr.contains("phantom-task"),
        "error must mention the unknown task_id: got '{stderr}'"
    );
}

// ── 8-5 ───────────────────────────────────────────────────────────────────────

/// A `{{vars.key}}` placeholder that has no matching entry in `vars` must be
/// preserved verbatim in the resolved prompt — unknown keys are not silently
/// dropped or replaced with an empty string.
#[test]
fn undefined_var_placeholder_preserved_in_prompt() {
    const CONFIG_UNDEF_VAR: &str = r#"
vars:
  known: make test

workflows:
  test-vars:
    name: Var test
    tasks:
      - id: work
        task: Do work
        prompt: "Run {{vars.known}} then {{vars.missing}}."
"#;
    let proj = TempProject::new(CONFIG_UNDEF_VAR);
    let out = proj.start("test-vars");

    let tasks = out["tasks"].as_array().expect("tasks must be an array");
    let prompt = tasks[0]["prompt"]
        .as_str()
        .expect("task must have a prompt");

    assert!(
        prompt.contains("make test"),
        "defined var must be expanded: got '{prompt}'"
    );
    assert!(
        prompt.contains("{{vars.missing}}"),
        "undefined var must be preserved verbatim: got '{prompt}'"
    );
}

// ── 8-6 ───────────────────────────────────────────────────────────────────────

/// A workflow defined in an imported file must be available to `start` and
/// appear in `list`, as if it had been defined inline in config.yml.
#[test]
fn imports_resolves_external_workflow() {
    let proj = TempProject::empty();

    // Write the imported workflow file first.
    proj.write_file(
        ".workflow/extra.yml",
        r#"
workflows:
  imported-wf:
    name: Imported Workflow
    tasks:
      - id: only-task
        task: The imported task
        prompt: Do the imported task.
"#,
    );

    // Main config imports it.
    proj.write_workflow_config(
        r#"
imports:
  - extra.yml

workflows:
  local-wf:
    name: Local Workflow
    tasks:
      - id: local-task
        task: The local task
        prompt: Do the local task.
"#,
    );

    // Both workflows must appear in `list`.
    let list = proj.list();
    let items = list.as_array().expect("list must be an array");
    let slugs: Vec<&str> = items.iter().map(|i| i["slug"].as_str().unwrap()).collect();
    assert!(slugs.contains(&"local-wf"), "local workflow must be listed");
    assert!(
        slugs.contains(&"imported-wf"),
        "imported workflow must be listed"
    );

    // The imported workflow must be startable.
    let out = proj.start("imported-wf");
    assert_eq!(out["status"], "started");
    assert_eq!(out["tasks"][0]["task_id"], "only-task");
}

// ── 8-7 ───────────────────────────────────────────────────────────────────────

/// A circular import chain (config.yml → a.yml → config.yml) must be detected
/// and reported.  `validate` exits 0 but returns `valid: false`; `start` exits 1.
#[test]
fn imports_circular_reference_is_detected() {
    let proj = TempProject::empty();

    // a.yml imports back to the root config — creates a cycle.
    proj.write_file(
        ".workflow/a.yml",
        r#"
imports:
  - config.yml

workflows:
  loop-wf:
    name: Loop
    tasks:
      - id: t
        task: t
        prompt: t
"#,
    );

    proj.write_workflow_config(
        r#"
imports:
  - a.yml

workflows:
  root-wf:
    name: Root
    tasks:
      - id: t
        task: t
        prompt: t
"#,
    );

    // `validate` must return valid:false with a circular-import error.
    let val_out = proj.validate();
    assert_eq!(val_out["valid"], false);
    let errors = val_out["errors"]
        .as_array()
        .expect("errors must be present");
    let combined: String = errors.iter().map(|e| e.as_str().unwrap_or("")).collect();
    assert!(
        combined.contains("circular"),
        "error must mention circular import: got '{combined}'"
    );

    // `start` must also fail (exit 1) for the same reason.
    let stderr = proj.assert_err(&["start", "root-wf"]);
    assert!(
        stderr.contains("circular"),
        "start error must mention circular import: got '{stderr}'"
    );
}

// ── 8-8 ───────────────────────────────────────────────────────────────────────

/// A diamond import pattern (A→B→shared, A→C→shared) must succeed.
/// The shared file is visited twice in separate DFS branches, which is
/// allowed because `visited` tracks the current DFS *stack*, not all nodes.
#[test]
fn imports_diamond_pattern_is_allowed() {
    let proj = TempProject::empty();

    proj.write_file(
        ".workflow/shared.yml",
        r#"
workflows:
  shared-wf:
    name: Shared Workflow
    tasks:
      - id: shared-task
        task: Shared task
        prompt: The shared task.
"#,
    );

    proj.write_file(
        ".workflow/b.yml",
        r#"
imports:
  - shared.yml
"#,
    );

    proj.write_file(
        ".workflow/c.yml",
        r#"
imports:
  - shared.yml
"#,
    );

    proj.write_workflow_config(
        r#"
imports:
  - b.yml
  - c.yml
"#,
    );

    // Diamond resolution must succeed — shared-wf must appear exactly once.
    let val_out = proj.validate();
    assert_eq!(
        val_out["valid"], true,
        "diamond imports must be valid: errors = {:?}",
        val_out["errors"]
    );

    let list = proj.list();
    let items = list.as_array().expect("list must be an array");
    assert_eq!(
        items.len(),
        1,
        "shared-wf must appear exactly once despite two import paths"
    );
    assert_eq!(items[0]["slug"], "shared-wf");
}
