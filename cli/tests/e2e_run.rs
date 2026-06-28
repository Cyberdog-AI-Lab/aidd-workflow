//! E2E tests for Scenario 9: `workflow-runner run` subcommand.
//!
//! These tests exercise the autonomous orchestrator mode where
//! `workflow-runner run` dispatches tasks to a (mock) Channels webhook and
//! advances the workflow via HTTP callbacks.
//!
//! Each test starts a `MockWebhook` to capture dispatch POSTs and drives the
//! workflow by POSTing to the callback server that `workflow-runner run` starts.
//!
//! Covers:
//!   9-1  initial tasks are dispatched to the webhook on startup
//!   9-2  linear workflow runs to completion via /complete callbacks
//!   9-3  tasks with `requires` are dispatched only after dependencies complete
//!   9-4  approval task pauses dispatch after /complete
//!   9-5  POST /next approves the paused task and resumes dispatch
//!   9-6  POST /reject/:id re-dispatches the rejected task
//!   9-7  POST /reject/:id with JSON reason body is accepted
//!   9-8  --callback-port changes the callback server bind port
//!   9-9  --callback-url sets callback_url in the webhook payload
//!   9-10 dispatch payload contains all required fields
//!   9-11 --webhook-port constructs the webhook URL from port number
//!   9-12 --callback-port and --callback-url together → clap conflict error
//!   9-13 --webhook-port and --webhook-url together → clap conflict error
//!   9-14 unknown workflow name → exit 1 with informative error
//!   9-15 webhook server unavailable → exit 1 with informative error

mod helpers;
use helpers::{pick_free_port, MockWebhook, TempProject, CONFIG_MINIMAL};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);

/// Two-task linear workflow: step-a → step-b.
const CONFIG_TWO_STEP: &str = r#"
workflows:
  two-step:
    name: Two Step
    tasks:
      - id: step-a
        task: Step A
        prompt: Do step A.
      - id: step-b
        task: Step B
        prompt: Do step B.
        requires: [step-a]
"#;

/// Workflow with an approval gate: work (approval: true) → finish.
const CONFIG_APPROVAL_RUN: &str = r#"
workflows:
  approve-run:
    name: Approve Run
    tasks:
      - id: work
        task: Do work
        prompt: Do the work.
        approval: true
      - id: finish
        task: Finish
        prompt: Finish up.
        requires: [work]
"#;

// ── 9-1 ───────────────────────────────────────────────────────────────────────

/// On startup, `workflow-runner run` must dispatch all initially executable
/// tasks (those with no `requires`) to the webhook before waiting.
#[test]
fn run_dispatches_initial_tasks() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_TWO_STEP);
    let cb_port = pick_free_port();
    let _proc = proj.start_run("two-step", cb_port, &webhook.url());

    let dispatched = webhook.wait_for_n(1, TIMEOUT);

    assert_eq!(
        dispatched[0]["task_id"].as_str(),
        Some("step-a"),
        "step-a (no requires) must be dispatched first"
    );
}

// ── 9-2 ───────────────────────────────────────────────────────────────────────

/// A two-task linear workflow must complete (process exits 0) when each task
/// is acknowledged with a /complete callback.
#[test]
fn run_linear_workflow_completes() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_TWO_STEP);
    let cb_port = pick_free_port();
    let mut proc = proj.start_run("two-step", cb_port, &webhook.url());

    webhook.wait_for_n(1, TIMEOUT);
    proc.complete("step-a");

    webhook.wait_for_n(2, TIMEOUT);
    proc.complete("step-b");

    let status = proc.wait_exit(TIMEOUT);
    assert!(
        status.success(),
        "workflow-runner run must exit 0 when all tasks complete"
    );
}

// ── 9-3 ───────────────────────────────────────────────────────────────────────

/// A task with `requires` must NOT be dispatched before its dependencies have
/// been acknowledged.  Once the dependency completes, it must be dispatched.
#[test]
fn run_requires_blocks_dispatch_until_dependency_done() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_TWO_STEP);
    let cb_port = pick_free_port();
    let proc = proj.start_run("two-step", cb_port, &webhook.url());

    // step-a has no requires → dispatched immediately
    webhook.wait_for_n(1, TIMEOUT);
    assert_eq!(
        webhook.count(),
        1,
        "only step-a must be dispatched initially"
    );

    // step-b requires step-a → must NOT be dispatched yet
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        webhook.count(),
        1,
        "step-b must not be dispatched before step-a completes"
    );

    // Complete step-a → step-b must now be dispatched
    proc.complete("step-a");

    let dispatched = webhook.wait_for_n(2, TIMEOUT);
    assert_eq!(
        dispatched[1]["task_id"].as_str(),
        Some("step-b"),
        "step-b must be dispatched after step-a completes"
    );
}

// ── 9-4 ───────────────────────────────────────────────────────────────────────

/// After a task with `approval: true` is completed, the workflow must pause
/// (no further tasks dispatched) until /next is called.
#[test]
fn run_approval_pauses_dispatch_after_complete() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_APPROVAL_RUN);
    let cb_port = pick_free_port();
    let proc = proj.start_run("approve-run", cb_port, &webhook.url());

    webhook.wait_for_n(1, TIMEOUT);
    assert_eq!(webhook.count(), 1);

    proc.complete("work");

    // finish must NOT be dispatched while the approval gate is active
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        webhook.count(),
        1,
        "finish must not be dispatched while workflow is awaiting approval"
    );
}

// ── 9-5 ───────────────────────────────────────────────────────────────────────

/// POST /next while in `awaiting_approval` must approve the task, dispatch the
/// next task, and allow the workflow to reach completion.
#[test]
fn run_next_approves_and_resumes_dispatch() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_APPROVAL_RUN);
    let cb_port = pick_free_port();
    let mut proc = proj.start_run("approve-run", cb_port, &webhook.url());

    webhook.wait_for_n(1, TIMEOUT);
    proc.complete("work");

    // Wait for approval gate to be set before calling /next
    std::thread::sleep(Duration::from_millis(100));
    proc.approve();

    // finish must be dispatched after approval
    let dispatched = webhook.wait_for_n(2, TIMEOUT);
    assert_eq!(
        dispatched[1]["task_id"].as_str(),
        Some("finish"),
        "finish must be dispatched after /next approves"
    );

    // Complete the final task → workflow exits
    proc.complete("finish");
    let status = proc.wait_exit(TIMEOUT);
    assert!(
        status.success(),
        "workflow must complete successfully after approval"
    );
}

// ── 9-6 ───────────────────────────────────────────────────────────────────────

/// POST /reject/:id while in `awaiting_approval` must clear the gate and
/// re-dispatch the rejected task to the webhook.
#[test]
fn run_reject_redispatches_task() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_APPROVAL_RUN);
    let cb_port = pick_free_port();
    let proc = proj.start_run("approve-run", cb_port, &webhook.url());

    webhook.wait_for_n(1, TIMEOUT);
    proc.complete("work");

    std::thread::sleep(Duration::from_millis(100));
    proc.reject("work");

    // work must be dispatched a second time
    let dispatched = webhook.wait_for_n(2, TIMEOUT);
    let ids: Vec<&str> = dispatched
        .iter()
        .map(|d| d["task_id"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        ids.iter().filter(|&&id| id == "work").count(),
        2,
        "work must be dispatched twice (initial + after reject)"
    );
}

// ── 9-7 ───────────────────────────────────────────────────────────────────────

/// POST /reject/:id with a JSON `{"reason": "..."}` body must be accepted and
/// result in the task being re-dispatched (reason is stored in DB, not echoed).
#[test]
fn run_reject_with_reason_body_is_accepted() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_APPROVAL_RUN);
    let cb_port = pick_free_port();
    let proc = proj.start_run("approve-run", cb_port, &webhook.url());

    webhook.wait_for_n(1, TIMEOUT);
    proc.complete("work");

    std::thread::sleep(Duration::from_millis(100));
    proc.reject_with_reason("work", "needs more detail");

    // work must be re-dispatched
    let dispatched = webhook.wait_for_n(2, TIMEOUT);
    assert_eq!(
        dispatched[1]["task_id"].as_str(),
        Some("work"),
        "work must be re-dispatched after reject with reason"
    );
}

// ── 9-8 ───────────────────────────────────────────────────────────────────────

/// `--callback-port <port>` must make the callback server bind on the specified
/// port.  This is implicit: if `complete()` succeeds (it POSTs to that port),
/// the server was listening there.
#[test]
fn run_custom_callback_port_binds_correctly() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_TWO_STEP);
    let cb_port = pick_free_port();

    // cb_port is OS-assigned (not the default 8789)
    let mut proc = proj.start_run("two-step", cb_port, &webhook.url());

    webhook.wait_for_n(1, TIMEOUT);
    proc.complete("step-a"); // succeeds only if callback server is on cb_port

    webhook.wait_for_n(2, TIMEOUT);
    proc.complete("step-b");

    let status = proc.wait_exit(TIMEOUT);
    assert!(status.success());
}

// ── 9-9 ───────────────────────────────────────────────────────────────────────

/// `--callback-url <url>` must set `callback_url` in the webhook dispatch
/// payload, regardless of where the local callback server actually binds.
#[test]
fn run_callback_url_option_sets_payload_url() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_TWO_STEP);
    let custom_url = "http://ngrok-tunnel.example.test:4040";

    // Use only --callback-url (conflicts with --callback-port so cannot combine).
    // The callback server binds on the default port 8789; only the payload URL differs.
    let mut child = proj.run_background(&[
        "run",
        "two-step",
        "--callback-url",
        custom_url,
        "--webhook-url",
        &webhook.url(),
    ]);

    let dispatched = webhook.wait_for_n(1, TIMEOUT);
    child.kill().ok();
    child.wait().ok();

    assert_eq!(
        dispatched[0]["callback_url"].as_str(),
        Some(custom_url),
        "webhook payload must carry the --callback-url value, not the local bind URL"
    );
}

// ── 9-10 ──────────────────────────────────────────────────────────────────────

/// The dispatch payload must contain all required fields:
/// task_id, task, prompt, callback_url, workflow_id, outputs, deny.
#[test]
fn run_dispatch_payload_contains_required_fields() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_TWO_STEP);
    let cb_port = pick_free_port();
    let _proc = proj.start_run("two-step", cb_port, &webhook.url());

    let dispatched = webhook.wait_for_n(1, TIMEOUT);
    let payload = &dispatched[0];

    assert!(
        payload["task_id"].is_string(),
        "payload must include task_id"
    );
    assert!(payload["task"].is_string(), "payload must include task");
    assert!(
        payload["callback_url"].is_string(),
        "payload must include callback_url"
    );
    assert!(
        payload["workflow_id"].is_string(),
        "payload must include workflow_id"
    );

    // outputs and deny default to null/empty when not configured
    assert!(
        payload.get("outputs").is_some(),
        "payload must include outputs key"
    );
    assert!(
        payload.get("deny").is_some(),
        "payload must include deny key"
    );
}

// ── 9-11 ──────────────────────────────────────────────────────────────────────

/// `--webhook-port <port>` must construct `http://127.0.0.1:<port>` and send
/// dispatches there (equivalent to `--webhook-url http://127.0.0.1:<port>`).
#[test]
fn run_webhook_port_constructs_url() {
    let webhook = MockWebhook::start();
    let proj = TempProject::new(CONFIG_TWO_STEP);
    let cb_port = pick_free_port();

    let mut child = proj.run_background(&[
        "run",
        "two-step",
        "--callback-port",
        &cb_port.to_string(),
        "--webhook-port",
        &webhook.port.to_string(),
    ]);

    let dispatched = webhook.wait_for_n(1, TIMEOUT);
    child.kill().ok();
    child.wait().ok();

    assert!(
        !dispatched.is_empty(),
        "dispatch must arrive at the --webhook-port target"
    );
}

// ── 9-12 ──────────────────────────────────────────────────────────────────────

/// Specifying both `--callback-port` and `--callback-url` must be rejected by
/// clap (they are mutually exclusive).
#[test]
fn run_callback_port_and_callback_url_conflict() {
    let proj = TempProject::new(CONFIG_MINIMAL);
    let out = proj.run(&[
        "run",
        "simple",
        "--callback-port",
        "9001",
        "--callback-url",
        "http://example.com:9001",
    ]);
    assert!(
        !out.status.success(),
        "--callback-port and --callback-url together must cause a non-zero exit"
    );
}

// ── 9-13 ──────────────────────────────────────────────────────────────────────

/// Specifying both `--webhook-port` and `--webhook-url` must be rejected by
/// clap (they are mutually exclusive).
#[test]
fn run_webhook_port_and_webhook_url_conflict() {
    let proj = TempProject::new(CONFIG_MINIMAL);
    let out = proj.run(&[
        "run",
        "simple",
        "--webhook-port",
        "9002",
        "--webhook-url",
        "http://example.com:9002",
    ]);
    assert!(
        !out.status.success(),
        "--webhook-port and --webhook-url together must cause a non-zero exit"
    );
}

// ── 9-14 ──────────────────────────────────────────────────────────────────────

/// Passing a workflow name that does not exist in config.yml must exit 1 and
/// include the unknown name in the error message.
#[test]
fn run_unknown_workflow_exits_with_error() {
    let proj = TempProject::new(CONFIG_TWO_STEP);
    let cb_port = pick_free_port();
    // webhook is never contacted because the process exits before dispatching
    let out = proj.run(&[
        "run",
        "no-such-workflow",
        "--callback-port",
        &cb_port.to_string(),
        "--webhook-url",
        "http://127.0.0.1:1",
    ]);

    assert!(!out.status.success(), "unknown workflow must exit non-zero");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no-such-workflow"),
        "error must mention the unknown workflow name: got '{stderr}'"
    );
}

// ── 9-15 ──────────────────────────────────────────────────────────────────────

/// When the Channels webhook server is not reachable, `workflow-runner run`
/// must exit 1 with an error that mentions the webhook.
#[test]
fn run_webhook_unavailable_exits_with_error() {
    let proj = TempProject::new(CONFIG_TWO_STEP);
    let cb_port = pick_free_port();
    let dead_port = pick_free_port(); // nothing is listening here

    let out = proj.run(&[
        "run",
        "two-step",
        "--callback-port",
        &cb_port.to_string(),
        "--webhook-url",
        &format!("http://127.0.0.1:{dead_port}"),
    ]);

    assert!(
        !out.status.success(),
        "unavailable webhook must cause a non-zero exit"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("webhook"),
        "error must mention 'webhook': got '{stderr}'"
    );
}
