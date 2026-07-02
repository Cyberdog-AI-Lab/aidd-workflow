//! E2E tests for Scenario 5: multiple concurrent workflows on one daemon.
//!
//! `workflow-runner serve` starts a single long-lived callback/orchestrator
//! daemon that does not itself run any workflow. Each `workflow-runner run
//! <workflow>` call (or raw `POST /run`) starts an independent workflow
//! instance on that daemon, identified by its own `workflow_id`. These tests
//! verify that a single daemon process correctly hosts several concurrent
//! workflow instances, that actions targeting one workflow never leak into
//! another, and that the daemon's lifecycle is decoupled from any individual
//! workflow's completion (it no longer auto-exits — see `ARCHITECTURE.md`).
//!
//! Covers:
//!   5-1  two `run` calls against one daemon get distinct workflow_ids and dispatches
//!   5-2  /complete for one workflow does not affect the other's state
//!   5-3  one workflow completing does not stop dispatch/complete for the other
//!   5-4  the daemon keeps running after all tracked workflows complete
//!   5-5  POST /stop shuts the daemon down gracefully
//!   5-6  /complete for an unknown workflow_id is ignored, not a crash
//!   5-7  `run` for a workflow name absent from config.yml fails without disturbing others
//!   5-8  approve/resume without --workflow-id errors when multiple candidates qualify
//!   5-9  approve/resume with an explicit --workflow-id picks the right one

mod helpers;
use helpers::{pick_free_port, wait_workflow_completed, MockWebhook, TempProject};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);

/// Two independent single-task workflows, so two `run` calls can be told apart
/// by which task_id they dispatch.
const CONFIG_TWO_WORKFLOWS: &str = r#"
workflows:
  alpha:
    name: Alpha Flow
    tasks:
      - id: alpha-task
        task: Alpha task
        prompt: Do the alpha task.
  beta:
    name: Beta Flow
    tasks:
      - id: beta-task
        task: Beta task
        prompt: Do the beta task.
"#;

/// A workflow with an approval gate, used to exercise ambiguous approve/resume
/// target resolution across multiple concurrent instances.
const CONFIG_APPROVAL: &str = r#"
workflows:
  sign-off:
    name: Sign-off Flow
    tasks:
      - id: work
        task: Do work
        prompt: Do the work.
        approval: true
"#;

// ── 5-1 ───────────────────────────────────────────────────────────────────────

/// Two `run` calls against the same `serve` daemon must be assigned distinct
/// workflow_ids and each dispatch its own initial task.
#[test]
fn two_run_calls_get_distinct_workflow_ids() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_TWO_WORKFLOWS);
    let cb_port = pick_free_port();
    let daemon = proj.start_daemon(cb_port, &webhook.url());

    let alpha_id = daemon.start_workflow("alpha");
    let beta_id = daemon.start_workflow("beta");

    assert_ne!(
        alpha_id, beta_id,
        "each /run call must be assigned a distinct workflow_id"
    );

    let dispatched = webhook.wait_for_n(2, TIMEOUT);
    let by_workflow_id: Vec<(&str, &str)> = dispatched
        .iter()
        .map(|d| {
            (
                d["workflow_id"].as_str().unwrap_or(""),
                d["task_id"].as_str().unwrap_or(""),
            )
        })
        .collect();
    assert!(
        by_workflow_id.contains(&(alpha_id.as_str(), "alpha-task")),
        "alpha's task must be dispatched tagged with alpha's workflow_id: got {by_workflow_id:?}"
    );
    assert!(
        by_workflow_id.contains(&(beta_id.as_str(), "beta-task")),
        "beta's task must be dispatched tagged with beta's workflow_id: got {by_workflow_id:?}"
    );
}

// ── 5-2 ───────────────────────────────────────────────────────────────────────

/// Completing a task on one workflow must not affect the other's dispatch
/// state — they are tracked independently by workflow_id.
#[test]
fn complete_on_one_workflow_does_not_affect_the_other() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_TWO_WORKFLOWS);
    let cb_port = pick_free_port();
    let daemon = proj.start_daemon(cb_port, &webhook.url());

    let alpha_id = daemon.start_workflow("alpha");
    let beta_id = daemon.start_workflow("beta");
    webhook.wait_for_n(2, TIMEOUT);

    daemon.complete_for(&alpha_id, "alpha-task");
    wait_workflow_completed(&proj, &alpha_id, TIMEOUT);

    // beta must still be active and untouched by alpha's completion.
    let out = proj.run(&["--workflow-id", &beta_id, "status"]);
    assert!(
        out.status.success(),
        "beta must still be active after alpha completed independently"
    );
}

// ── 5-3 ───────────────────────────────────────────────────────────────────────

/// One workflow completing must not prevent the other from continuing to
/// dispatch and complete normally on the same daemon.
#[test]
fn one_workflow_completing_does_not_stop_the_other() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_TWO_WORKFLOWS);
    let cb_port = pick_free_port();
    let daemon = proj.start_daemon(cb_port, &webhook.url());

    let alpha_id = daemon.start_workflow("alpha");
    let beta_id = daemon.start_workflow("beta");
    webhook.wait_for_n(2, TIMEOUT);

    daemon.complete_for(&alpha_id, "alpha-task");
    wait_workflow_completed(&proj, &alpha_id, TIMEOUT);

    // beta must still be completable afterward.
    daemon.complete_for(&beta_id, "beta-task");
    wait_workflow_completed(&proj, &beta_id, TIMEOUT);
}

// ── 5-4 ───────────────────────────────────────────────────────────────────────

/// The daemon must keep running (accepting further requests) even after every
/// workflow it was tracking has completed — it no longer auto-exits.
#[test]
fn daemon_keeps_running_after_all_workflows_complete() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_TWO_WORKFLOWS);
    let cb_port = pick_free_port();
    let mut daemon = proj.start_daemon(cb_port, &webhook.url());

    let alpha_id = daemon.start_workflow("alpha");
    webhook.wait_for_n(1, TIMEOUT);
    daemon.complete_for(&alpha_id, "alpha-task");
    wait_workflow_completed(&proj, &alpha_id, TIMEOUT);

    // The daemon process itself must still be alive.
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        daemon.try_wait().is_none(),
        "serve must not exit automatically once its workflows complete"
    );

    // ...and still able to accept a brand new workflow.
    let beta_id = daemon.start_workflow("beta");
    webhook.wait_for_n(2, TIMEOUT);
    daemon.complete_for(&beta_id, "beta-task");
    wait_workflow_completed(&proj, &beta_id, TIMEOUT);
}

// ── 5-5 ───────────────────────────────────────────────────────────────────────

/// `POST /stop` must cause the daemon process to exit.
#[test]
fn stop_shuts_the_daemon_down() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_TWO_WORKFLOWS);
    let cb_port = pick_free_port();
    let mut daemon = proj.start_daemon(cb_port, &webhook.url());

    // Make sure the daemon is actually up before stopping it.
    daemon.start_workflow("alpha");
    webhook.wait_for_n(1, TIMEOUT);

    daemon.stop();
    let status = daemon.wait_exit(TIMEOUT);
    assert!(status.success(), "serve must exit 0 after POST /stop");
}

// ── 5-6 ───────────────────────────────────────────────────────────────────────

/// `/complete` for a workflow_id the daemon has never seen must be ignored
/// (logged server-side) rather than crashing the daemon or disturbing other
/// workflows it is tracking.
#[test]
fn complete_for_unknown_workflow_id_is_ignored_not_a_crash() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_TWO_WORKFLOWS);
    let cb_port = pick_free_port();
    let daemon = proj.start_daemon(cb_port, &webhook.url());

    let alpha_id = daemon.start_workflow("alpha");
    webhook.wait_for_n(1, TIMEOUT);

    daemon.complete_for("00000000-0000-0000-0000-000000000000", "alpha-task");

    // No new dispatch should occur, and the daemon must still be responsive
    // to the real workflow it is tracking afterward.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        webhook.count(),
        1,
        "an unknown workflow_id must not cause any dispatch"
    );

    daemon.complete_for(&alpha_id, "alpha-task");
    wait_workflow_completed(&proj, &alpha_id, TIMEOUT);
}

// ── 5-7 ───────────────────────────────────────────────────────────────────────

/// Starting a workflow name that isn't defined in config.yml must fail (the
/// `/run` response is a client error) without disturbing an already-running
/// workflow on the same daemon.
#[test]
fn run_unknown_workflow_name_fails_without_disturbing_others() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_TWO_WORKFLOWS);
    let cb_port = pick_free_port();
    let daemon = proj.start_daemon(cb_port, &webhook.url());

    let alpha_id = daemon.start_workflow("alpha");
    webhook.wait_for_n(1, TIMEOUT);

    let out = proj.run_retrying(
        &[
            "run",
            "no-such-workflow",
            "--callback-port",
            &cb_port.to_string(),
        ],
        TIMEOUT,
    );
    assert!(
        !out.status.success(),
        "starting an undefined workflow name must fail"
    );

    // alpha must be completely unaffected.
    daemon.complete_for(&alpha_id, "alpha-task");
    wait_workflow_completed(&proj, &alpha_id, TIMEOUT);
}

// ── 5-8 ───────────────────────────────────────────────────────────────────────

/// `approve`/`resume` without `--workflow-id` must error out (listing
/// candidates) when more than one workflow qualifies for the target status,
/// rather than guessing which one to act on.
#[test]
fn approve_without_workflow_id_errors_when_ambiguous() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_APPROVAL);
    let cb_port = pick_free_port();
    let daemon = proj.start_daemon(cb_port, &webhook.url());

    let id_a = daemon.start_workflow("sign-off");
    let id_b = daemon.start_workflow("sign-off");
    webhook.wait_for_n(2, TIMEOUT);

    daemon.complete_for(&id_a, "work"); // approval: true → awaiting_approval
    daemon.complete_for(&id_b, "work");
    std::thread::sleep(Duration::from_millis(150));

    let stderr = proj.assert_err(&["approve", "--callback-port", &cb_port.to_string()]);
    assert!(
        stderr.contains(&id_a) && stderr.contains(&id_b),
        "ambiguous approve error must list both candidate workflow_ids: got '{stderr}'"
    );
}

// ── 5-9 ───────────────────────────────────────────────────────────────────────

/// `approve --workflow-id <id>` must target exactly that workflow even when
/// another workflow is also awaiting approval on the same daemon.
#[test]
fn approve_with_explicit_workflow_id_targets_the_right_one() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_APPROVAL);
    let cb_port = pick_free_port();
    let daemon = proj.start_daemon(cb_port, &webhook.url());

    let id_a = daemon.start_workflow("sign-off");
    let id_b = daemon.start_workflow("sign-off");
    webhook.wait_for_n(2, TIMEOUT);

    daemon.complete_for(&id_a, "work");
    daemon.complete_for(&id_b, "work");
    std::thread::sleep(Duration::from_millis(150));

    let out = proj.assert_ok(&[
        "--workflow-id",
        &id_a,
        "approve",
        "--callback-port",
        &cb_port.to_string(),
    ]);
    assert_eq!(out["workflow_id"], id_a);

    // id_a (single-task, now approved) must complete; id_b must remain
    // awaiting_approval, untouched by the targeted approve.
    wait_workflow_completed(&proj, &id_a, TIMEOUT);
    let out = proj.run(&["--workflow-id", &id_b, "status"]);
    assert!(
        out.status.success(),
        "id_b must still exist (still awaiting_approval), untouched by the targeted approve"
    );
}
