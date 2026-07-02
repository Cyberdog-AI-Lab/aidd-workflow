//! E2E tests for Scenario 8: error cases and edge-case behaviours.
//!
//! Covers:
//!   8-5  undefined {{vars.key}} placeholders are preserved verbatim in prompts
//!   8-6  imports: an external workflow file is resolved and merged
//!   8-7  imports: a circular reference is detected and reported
//!   8-8  imports: diamond imports (A→B→shared, A→C→shared) are allowed
//!
//! Error cases for the removed manual `start`/`next`/`report`/`complete`
//! commands no longer apply (those commands were removed in favor of the
//! `run` daemon). The `complete` gate-rejection case (unknown task_id →
//! allowed:false, not a crash) now lives in `e2e_run.rs` since it requires a
//! running daemon to exercise via `/complete/:task_id`.

mod helpers;
use helpers::{pick_free_port, MockWebhook, TempProject};
use std::time::Duration;

// ── 8-5 ───────────────────────────────────────────────────────────────────────

/// A `{{vars.key}}` placeholder that has no matching entry in `vars` must be
/// preserved verbatim in the resolved prompt — unknown keys are not silently
/// dropped or replaced with an empty string. Prompt expansion only happens
/// via `executor::build_next`, which is only reachable through the `run`
/// daemon now, so this drives one to inspect the dispatched payload.
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
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_UNDEF_VAR);
    let cb_port = pick_free_port();
    let _proc = proj.start_run("test-vars", cb_port, &webhook.url());

    let dispatched = webhook.wait_for_n(1, Duration::from_secs(10));
    let prompt = dispatched[0]["prompt"]
        .as_str()
        .expect("work must have a prompt");

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

/// A workflow defined in an imported file must be available and appear in
/// `list`, as if it had been defined inline in config.yml.
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

    // The imported config must also pass validation.
    let val_out = proj.validate();
    assert_eq!(val_out["valid"], true, "errors = {:?}", val_out["errors"]);
}

// ── 8-7 ───────────────────────────────────────────────────────────────────────

/// A circular import chain (config.yml → a.yml → config.yml) must be detected
/// and reported. `validate` exits 0 but returns `valid: false`.
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
