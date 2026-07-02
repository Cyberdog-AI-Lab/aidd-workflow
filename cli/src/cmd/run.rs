use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{
    extract::{Path as AxumPath, State},
    http::StatusCode,
    routing::post,
    Router,
};
use chrono::Utc;
use serde::Deserialize;
use tokio::sync::{mpsc, oneshot};

use crate::config::loader::load_config;
use crate::config::types::{Config, Workflow};
use crate::engine::state::{ActionReport, StepStatus};
use crate::engine::{dag, executor, gate, store};
use crate::protocol::input::{CompleteInput, PauseInput, ReportInput, RunInput};
use crate::protocol::output::{ErrorOutput, FlowStatus, RunOutput, TaskOutput};

enum RunEvent {
    Run {
        workflow_name: String,
        respond_to: oneshot::Sender<Result<RunOutput, String>>,
    },
    Stop,
    Complete {
        workflow_id: String,
        task_id: String,
        summary: Option<String>,
    },
    Report {
        workflow_id: String,
        task_id: String,
        body: String,
    },
    Approve {
        workflow_id: String,
    },
    Resume {
        workflow_id: String,
    },
    Reject {
        workflow_id: String,
        task_id: String,
        reason: Option<String>,
    },
    Pause {
        workflow_id: String,
        task_id: String,
        reason: Option<String>,
    },
}

struct AppState {
    tx: mpsc::Sender<RunEvent>,
}

/// A single workflow instance currently tracked by the daemon.
struct RunningWorkflow {
    wf: Workflow,
    dispatched: HashSet<String>,
}

#[derive(Deserialize)]
struct RejectBody {
    reason: Option<String>,
}

async fn handle_run(State(state): State<Arc<AppState>>, body: String) -> (StatusCode, String) {
    let workflow_name = match serde_json::from_str::<RunInput>(&body) {
        Ok(input) => input.workflow,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                serde_json::to_string(&ErrorOutput {
                    error: format!("invalid request body: {}", e),
                })
                .unwrap(),
            );
        }
    };

    let (resp_tx, resp_rx) = oneshot::channel();
    if state
        .tx
        .send(RunEvent::Run {
            workflow_name,
            respond_to: resp_tx,
        })
        .await
        .is_err()
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::to_string(&ErrorOutput {
                error: "daemon event loop is not running".to_string(),
            })
            .unwrap(),
        );
    }

    match resp_rx.await {
        Ok(Ok(output)) => (StatusCode::OK, serde_json::to_string(&output).unwrap()),
        Ok(Err(e)) => (
            StatusCode::BAD_REQUEST,
            serde_json::to_string(&ErrorOutput { error: e }).unwrap(),
        ),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::to_string(&ErrorOutput {
                error: "daemon did not respond".to_string(),
            })
            .unwrap(),
        ),
    }
}

async fn handle_stop(State(state): State<Arc<AppState>>) -> &'static str {
    state.tx.send(RunEvent::Stop).await.ok();
    "ok"
}

async fn handle_complete(
    AxumPath((workflow_id, task_id)): AxumPath<(String, String)>,
    State(state): State<Arc<AppState>>,
    body: String,
) -> &'static str {
    let summary = serde_json::from_str::<CompleteInput>(&body)
        .ok()
        .and_then(|b| b.summary);
    state
        .tx
        .send(RunEvent::Complete {
            workflow_id,
            task_id,
            summary,
        })
        .await
        .ok();
    "ok"
}

async fn handle_report(
    AxumPath((workflow_id, task_id)): AxumPath<(String, String)>,
    State(state): State<Arc<AppState>>,
    body: String,
) -> &'static str {
    state
        .tx
        .send(RunEvent::Report {
            workflow_id,
            task_id,
            body,
        })
        .await
        .ok();
    "ok"
}

async fn handle_approve(
    AxumPath(workflow_id): AxumPath<String>,
    State(state): State<Arc<AppState>>,
) -> &'static str {
    state.tx.send(RunEvent::Approve { workflow_id }).await.ok();
    "ok"
}

async fn handle_resume(
    AxumPath(workflow_id): AxumPath<String>,
    State(state): State<Arc<AppState>>,
) -> &'static str {
    state.tx.send(RunEvent::Resume { workflow_id }).await.ok();
    "ok"
}

async fn handle_reject(
    AxumPath((workflow_id, task_id)): AxumPath<(String, String)>,
    State(state): State<Arc<AppState>>,
    body: String,
) -> &'static str {
    let reason = serde_json::from_str::<RejectBody>(&body)
        .ok()
        .and_then(|b| b.reason);
    state
        .tx
        .send(RunEvent::Reject {
            workflow_id,
            task_id,
            reason,
        })
        .await
        .ok();
    "ok"
}

async fn handle_pause(
    AxumPath((workflow_id, task_id)): AxumPath<(String, String)>,
    State(state): State<Arc<AppState>>,
    body: String,
) -> &'static str {
    let reason = serde_json::from_str::<PauseInput>(&body)
        .ok()
        .and_then(|b| b.reason);
    state
        .tx
        .send(RunEvent::Pause {
            workflow_id,
            task_id,
            reason,
        })
        .await
        .ok();
    "ok"
}

/// Starts the callback HTTP server and blocks, managing zero or more concurrent
/// workflow instances. Workflows are added via `POST /run` and removed once they
/// complete. The daemon keeps running (to accept further `/run` calls) until it
/// receives `POST /stop` or the process is killed — it does NOT exit automatically
/// when all tracked workflows finish.
pub async fn run_daemon(
    cwd: PathBuf,
    callback_port: u16,
    callback_url: String,
    webhook_url: String,
) -> Result<()> {
    let (tx, mut rx) = mpsc::channel::<RunEvent>(32);

    let app_state = Arc::new(AppState { tx });
    let app = Router::new()
        .route("/run", post(handle_run))
        .route("/stop", post(handle_stop))
        // `*task_id` (not `:task_id`) because sub-agent task IDs contain a
        // literal `/` (e.g. "quality-check/run-test"); a single-segment
        // param would not match those paths.
        .route("/complete/:workflow_id/*task_id", post(handle_complete))
        .route("/report/:workflow_id/*task_id", post(handle_report))
        .route("/approve/:workflow_id", post(handle_approve))
        .route("/resume/:workflow_id", post(handle_resume))
        .route("/reject/:workflow_id/*task_id", post(handle_reject))
        .route("/pause/:workflow_id/*task_id", post(handle_pause))
        .with_state(app_state);

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", callback_port))
        .await
        .context("failed to bind callback server")?;
    eprintln!(
        "[serve] callback server listening on 127.0.0.1:{}",
        callback_port
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Fail fast if config.yml is missing or invalid, before accepting any /run calls.
    let config = load_config(&cwd)?;
    let mut running: HashMap<String, RunningWorkflow> = HashMap::new();

    loop {
        match rx.recv().await {
            None => break,

            Some(RunEvent::Stop) => {
                eprintln!("[serve] stop requested; shutting down");
                break;
            }

            Some(RunEvent::Run {
                workflow_name,
                respond_to,
            }) => {
                let result = start_workflow(
                    &cwd,
                    &workflow_name,
                    &config,
                    &callback_url,
                    &webhook_url,
                    &mut running,
                )
                .await
                .map_err(|e| e.to_string());
                respond_to.send(result).ok();
            }

            Some(RunEvent::Report {
                workflow_id,
                task_id,
                body,
            }) => {
                if !running.contains_key(&workflow_id) {
                    eprintln!("[serve] /report for unknown workflow_id '{}'", workflow_id);
                    continue;
                }
                if let Err(e) = record_report(&cwd, &workflow_id, &task_id, &body) {
                    eprintln!(
                        "[serve] report error for '{}' (workflow {}): {}",
                        task_id, workflow_id, e
                    );
                }
            }

            Some(RunEvent::Complete {
                workflow_id,
                task_id,
                summary,
            }) => {
                let Some(wf) = running.get(&workflow_id).map(|e| e.wf.clone()) else {
                    eprintln!(
                        "[serve] /complete for unknown workflow_id '{}'",
                        workflow_id
                    );
                    continue;
                };
                if let Some(entry) = running.get_mut(&workflow_id) {
                    entry.dispatched.remove(&task_id);
                }
                eprintln!("[serve] [{}] task '{}' completed", workflow_id, task_id);

                let next_tasks =
                    complete_task(&cwd, &workflow_id, &task_id, summary, &wf, &config)?;
                if let Some(tasks) = next_tasks {
                    if let Some(entry) = running.get_mut(&workflow_id) {
                        dispatch_tasks(
                            &cwd,
                            &tasks,
                            &callback_url,
                            &webhook_url,
                            &workflow_id,
                            &mut entry.dispatched,
                        )
                        .await?;
                    }
                }

                // Check if the workflow record was cleared (completed) or paused
                match store::load_state_by_id(&cwd, &workflow_id)? {
                    None => {
                        eprintln!(
                            "[serve] workflow '{}' (id: {}) completed",
                            wf.name, workflow_id
                        );
                        running.remove(&workflow_id);
                    }
                    Some(_) => {
                        let wf_status = store::get_workflow_status(&cwd, &workflow_id)?;
                        if wf_status == "awaiting_approval" {
                            eprintln!(
                                "[serve] [{}] paused awaiting approval for task '{}'; \
                                 POST /approve/{} to approve or /reject/{}/{} to retry",
                                workflow_id, task_id, workflow_id, workflow_id, task_id
                            );
                        }
                    }
                }
            }

            Some(RunEvent::Approve { workflow_id }) => {
                let Some(wf) = running.get(&workflow_id).map(|e| e.wf.clone()) else {
                    eprintln!("[serve] /approve for unknown workflow_id '{}'", workflow_id);
                    continue;
                };
                let wf_status = store::get_workflow_status(&cwd, &workflow_id)?;
                if wf_status != "awaiting_approval" {
                    eprintln!(
                        "[serve] /approve called but workflow '{}' is not awaiting approval (status: {})",
                        workflow_id, wf_status
                    );
                    continue;
                }
                store::set_workflow_status(&cwd, &workflow_id, "active")?;

                eprintln!("[serve] [{}] approved; dispatching next tasks", workflow_id);

                let state = store::load_state_by_id(&cwd, &workflow_id)?
                    .context("workflow state not found after approval")?;
                let next = executor::build_next(&wf, &state, &config);
                if matches!(next.status, FlowStatus::Completed) {
                    store::clear_state_by_id(&cwd, &workflow_id)?;
                    eprintln!(
                        "[serve] workflow '{}' (id: {}) completed after approval",
                        wf.name, workflow_id
                    );
                    running.remove(&workflow_id);
                    continue;
                }
                if let Some(entry) = running.get_mut(&workflow_id) {
                    dispatch_tasks(
                        &cwd,
                        &next.tasks,
                        &callback_url,
                        &webhook_url,
                        &workflow_id,
                        &mut entry.dispatched,
                    )
                    .await?;
                }
            }

            Some(RunEvent::Resume { workflow_id }) => {
                let Some(wf) = running.get(&workflow_id).map(|e| e.wf.clone()) else {
                    eprintln!("[serve] /resume for unknown workflow_id '{}'", workflow_id);
                    continue;
                };
                let wf_status = store::get_workflow_status(&cwd, &workflow_id)?;
                if wf_status != "paused" {
                    eprintln!(
                        "[serve] /resume called but workflow '{}' is not paused (status: {})",
                        workflow_id, wf_status
                    );
                    continue;
                }
                store::set_workflow_status(&cwd, &workflow_id, "active")?;

                // Re-dispatch in_progress tasks that were paused. Clear them from
                // `dispatched` first — they were marked dispatched on their initial
                // send and dispatch_tasks() skips anything already in that set.
                let state = store::load_state_by_id(&cwd, &workflow_id)?
                    .context("workflow state not found after resume")?;
                let in_progress: Vec<TaskOutput> = state
                    .tasks
                    .iter()
                    .filter(|(_, s)| s.status == StepStatus::InProgress)
                    .filter_map(|(id, _)| build_task_output(id, &wf, &config))
                    .collect();
                if let Some(entry) = running.get_mut(&workflow_id) {
                    for task in &in_progress {
                        entry.dispatched.remove(&task.task_id);
                    }
                }
                eprintln!(
                    "[serve] [{}] resumed; re-dispatching in-progress tasks",
                    workflow_id
                );
                if let Some(entry) = running.get_mut(&workflow_id) {
                    dispatch_tasks(
                        &cwd,
                        &in_progress,
                        &callback_url,
                        &webhook_url,
                        &workflow_id,
                        &mut entry.dispatched,
                    )
                    .await?;
                }
            }

            Some(RunEvent::Reject {
                workflow_id,
                task_id,
                reason,
            }) => {
                let Some(wf) = running.get(&workflow_id).map(|e| e.wf.clone()) else {
                    eprintln!("[serve] /reject for unknown workflow_id '{}'", workflow_id);
                    continue;
                };
                let wf_status = store::get_workflow_status(&cwd, &workflow_id)?;
                if wf_status != "awaiting_approval" {
                    eprintln!(
                        "[serve] /reject called but workflow '{}' is not awaiting approval",
                        workflow_id
                    );
                    continue;
                }

                let mut state = store::load_state_by_id(&cwd, &workflow_id)?
                    .context("workflow state not found")?;

                {
                    let s = state.tasks.entry(task_id.clone()).or_default();
                    s.status = StepStatus::InProgress;
                    s.completed_at = None;
                    if let Some(ref r) = reason {
                        s.action_reports.push(ActionReport {
                            action_index: s.action_reports.len(),
                            action_type: "reject".to_string(),
                            exit_code: None,
                            stdout: Some(r.clone()),
                            recorded_at: Utc::now(),
                        });
                    }
                }
                store::set_workflow_status(&cwd, &workflow_id, "active")?;
                store::save_state(&cwd, &state)?;
                eprintln!(
                    "[serve] [{}] task '{}' rejected; re-dispatching",
                    workflow_id, task_id
                );

                let task_output = build_task_output(&task_id, &wf, &config);
                if let Some(t) = task_output {
                    if let Some(entry) = running.get_mut(&workflow_id) {
                        entry.dispatched.remove(&task_id);
                        dispatch_tasks(
                            &cwd,
                            &[t],
                            &callback_url,
                            &webhook_url,
                            &workflow_id,
                            &mut entry.dispatched,
                        )
                        .await?;
                    }
                }
            }

            Some(RunEvent::Pause {
                workflow_id,
                task_id,
                reason,
            }) => {
                if !running.contains_key(&workflow_id) {
                    eprintln!("[serve] /pause for unknown workflow_id '{}'", workflow_id);
                    continue;
                }
                store::set_workflow_status(&cwd, &workflow_id, "paused")?;

                // Record the pause event in action_reports.
                if let Ok(Some(mut state)) = store::load_state_by_id(&cwd, &workflow_id) {
                    let s = state.tasks.entry(task_id.clone()).or_default();
                    s.action_reports.push(ActionReport {
                        action_index: s.action_reports.len(),
                        action_type: "pause".to_string(),
                        exit_code: None,
                        stdout: reason.clone(),
                        recorded_at: Utc::now(),
                    });
                    store::save_state(&cwd, &state).ok();
                }
                store::update_task_timestamp(&cwd, &workflow_id, &task_id).ok();

                eprintln!(
                    "[serve] [{}] task '{}' paused — user input required: {}",
                    workflow_id,
                    task_id,
                    reason.as_deref().unwrap_or("(no reason given)")
                );
            }
        }
    }

    Ok(())
}

/// Creates a new workflow instance, persists its initial state, dispatches its
/// first executable tasks, and registers it in `running`. Called from the `/run`
/// event so a single daemon can host any number of concurrent workflow instances.
async fn start_workflow(
    cwd: &Path,
    workflow_name: &str,
    config: &Config,
    callback_url: &str,
    webhook_url: &str,
    running: &mut HashMap<String, RunningWorkflow>,
) -> Result<RunOutput> {
    let wf = config
        .workflows
        .get(workflow_name)
        .with_context(|| format!("workflow '{}' not found in config.yml", workflow_name))?
        .clone();

    let state = crate::engine::state::WorkflowState::new(workflow_name, &wf);
    let workflow_id = state.workflow_id.clone();
    store::save_state(cwd, &state)?;
    eprintln!(
        "[serve] workflow '{}' started (id: {})",
        workflow_name, workflow_id
    );

    let mut entry = RunningWorkflow {
        wf,
        dispatched: HashSet::new(),
    };
    let initial = executor::build_next(&entry.wf, &state, config);
    dispatch_tasks(
        cwd,
        &initial.tasks,
        callback_url,
        webhook_url,
        &workflow_id,
        &mut entry.dispatched,
    )
    .await?;

    let workflow = workflow_name.to_string();
    running.insert(workflow_id.clone(), entry);
    Ok(RunOutput {
        workflow_id,
        workflow,
    })
}

/// Marks a task as completed, enforces the gate check, handles the approval flag, and
/// returns the next set of tasks to dispatch.  Returns None when the workflow has
/// finished (cleared from the store) or has paused waiting for approval.
pub fn complete_task(
    cwd: &Path,
    workflow_id: &str,
    task_id: &str,
    summary: Option<String>,
    wf: &Workflow,
    config: &Config,
) -> Result<Option<Vec<TaskOutput>>> {
    let mut state =
        store::load_state_by_id(cwd, workflow_id)?.context("workflow state not found")?;

    let gate_result = gate::check(wf, &state, task_id);
    if !gate_result.allowed {
        eprintln!(
            "[run] gate check failed for '{}': {:?}",
            task_id, gate_result.reason
        );
        return Ok(Some(vec![]));
    }

    {
        let s = state.tasks.entry(task_id.to_string()).or_default();
        s.status = StepStatus::Completed;
        s.completed_at = Some(Utc::now());
        s.action_reports.push(ActionReport {
            action_index: s.action_reports.len(),
            action_type: "complete".to_string(),
            exit_code: None,
            stdout: summary,
            recorded_at: Utc::now(),
        });
    }

    if let Some(parent_id) = dag::parent_of(task_id) {
        state.sync_agents_parent(parent_id, wf)?;
    }

    store::save_state(cwd, &state)?;

    // Check approval gate (only for non-agent tasks)
    if dag::parent_of(task_id).is_none() {
        if let Some(task_cfg) = wf.tasks.iter().find(|t| t.id == task_id) {
            if task_cfg.approval {
                store::set_workflow_status(cwd, workflow_id, "awaiting_approval")?;
                return Ok(None);
            }
        }
    }

    let next = executor::build_next(wf, &state, config);
    if matches!(next.status, FlowStatus::Completed) {
        store::clear_state_by_id(cwd, workflow_id)?;
        return Ok(None);
    }

    Ok(Some(next.tasks))
}

/// Applies an intermediate action report from the Claude Code worker to the stored state.
/// Does not change task status; status transitions happen via dispatch (InProgress) and complete.
pub fn record_report(cwd: &Path, workflow_id: &str, task_id: &str, body: &str) -> Result<()> {
    let mut state =
        store::load_state_by_id(cwd, workflow_id)?.context("workflow state not found")?;

    if !state.tasks.contains_key(task_id) {
        anyhow::bail!("unknown task_id '{}' in workflow", task_id);
    }

    let s = state.tasks.entry(task_id.to_string()).or_default();
    let summary = serde_json::from_str::<ReportInput>(body)
        .ok()
        .and_then(|i| i.summary);
    s.action_reports.push(ActionReport {
        action_index: s.action_reports.len(),
        action_type: "report".to_string(),
        exit_code: None,
        stdout: summary,
        recorded_at: Utc::now(),
    });

    store::save_state(cwd, &state)?;
    store::update_task_timestamp(cwd, workflow_id, task_id)?;
    Ok(())
}

fn build_task_output(task_id: &str, wf: &Workflow, config: &Config) -> Option<TaskOutput> {
    wf.tasks
        .iter()
        .find(|t| t.id == task_id)
        .map(|task| TaskOutput {
            task_id: task.id.clone(),
            task: task.task.clone(),
            prompt: task
                .prompt
                .as_deref()
                .map(|p| executor::resolve_template(p, config)),
            skills: task.skills.clone(),
            agents: task.agents.clone(),
            outputs: task.outputs.clone(),
            deny: task.deny.clone(),
            approval: task.approval,
        })
}

async fn dispatch_tasks(
    cwd: &Path,
    tasks: &[TaskOutput],
    callback_url: &str,
    webhook_url: &str,
    workflow_id: &str,
    dispatched: &mut HashSet<String>,
) -> Result<()> {
    let client = reqwest::Client::new();
    for task in tasks {
        if dispatched.contains(&task.task_id) {
            continue;
        }
        store::mark_task_dispatched(cwd, workflow_id, &task.task_id)?;
        let payload = serde_json::json!({
            "task_id": task.task_id,
            "task": task.task,
            "prompt": task.prompt,
            "skills": task.skills,
            "agents": task.agents,
            "callback_url": callback_url,
            "workflow_id": workflow_id,
            "outputs": task.outputs,
            "deny": task.deny,
        });
        eprintln!("[serve] dispatching task '{}' to webhook", task.task_id);
        client
            .post(webhook_url)
            .json(&payload)
            .send()
            .await
            .with_context(|| {
                format!(
                    "failed to POST task '{}' to channels webhook ({}); \
                     is channels/webhook.ts running?",
                    task.task_id, webhook_url
                )
            })?;
        dispatched.insert(task.task_id.clone());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{Task, Workflow};
    use crate::engine::state::WorkflowState;
    use tempfile::TempDir;

    fn linear_workflow() -> Workflow {
        Workflow {
            name: "test".to_string(),
            description: None,
            tasks: vec![
                Task {
                    id: "step-a".to_string(),
                    task: Some("Step A".to_string()),
                    prompt: Some("Do step A".to_string()),
                    ..Task::default()
                },
                Task {
                    id: "step-b".to_string(),
                    task: Some("Step B".to_string()),
                    prompt: Some("Do step B".to_string()),
                    requires: vec!["step-a".to_string()],
                    ..Task::default()
                },
            ],
        }
    }

    fn approval_workflow() -> Workflow {
        Workflow {
            name: "test".to_string(),
            description: None,
            tasks: vec![Task {
                id: "review".to_string(),
                task: Some("Review".to_string()),
                prompt: Some("Review this".to_string()),
                approval: true,
                ..Task::default()
            }],
        }
    }

    fn empty_config(wf: Workflow) -> Config {
        let mut workflows = std::collections::HashMap::new();
        workflows.insert("test".to_string(), wf);
        Config {
            imports: vec![],
            vars: std::collections::HashMap::new(),
            workflows,
        }
    }

    #[test]
    fn complete_task_advances_to_next() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path();
        let wf = linear_workflow();
        let config = empty_config(wf.clone());

        let state = WorkflowState::new("test", &wf);
        let id = state.workflow_id.clone();
        store::save_state(cwd, &state).unwrap();

        let next = complete_task(cwd, &id, "step-a", None, &wf, &config)
            .unwrap()
            .unwrap();
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].task_id, "step-b");
    }

    #[test]
    fn complete_task_returns_none_when_all_done() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path();
        let wf = linear_workflow();
        let config = empty_config(wf.clone());

        let mut state = WorkflowState::new("test", &wf);
        let id = state.workflow_id.clone();
        state.tasks.get_mut("step-a").unwrap().status = StepStatus::Completed;
        store::save_state(cwd, &state).unwrap();

        let result = complete_task(cwd, &id, "step-b", None, &wf, &config).unwrap();
        // None means workflow finished (cleared from store)
        assert!(result.is_none());
        assert!(store::load_state_by_id(cwd, &id).unwrap().is_none());
    }

    #[test]
    fn complete_task_sets_awaiting_approval() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path();
        let wf = approval_workflow();
        let config = empty_config(wf.clone());

        let state = WorkflowState::new("test", &wf);
        let id = state.workflow_id.clone();
        store::save_state(cwd, &state).unwrap();

        let result = complete_task(cwd, &id, "review", None, &wf, &config).unwrap();
        assert!(result.is_none());
        assert_eq!(
            store::get_workflow_status(cwd, &id).unwrap(),
            "awaiting_approval"
        );
    }

    #[test]
    fn record_report_appends_action_report() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path();
        let wf = linear_workflow();

        let state = WorkflowState::new("test", &wf);
        let id = state.workflow_id.clone();
        store::save_state(cwd, &state).unwrap();

        let body = serde_json::json!({
            "session_id": "s",
            "task_id": "step-a",
            "action_index": 0,
            "action_type": "bash",
            "exit_code": 0,
            "stdout": "done"
        })
        .to_string();

        record_report(cwd, &id, "step-a", &body).unwrap();

        let loaded = store::load_state_by_id(cwd, &id).unwrap().unwrap();
        // Status remains Pending; InProgress is now set by dispatch (mark_task_dispatched).
        assert_eq!(loaded.tasks["step-a"].status, StepStatus::Pending);
        assert_eq!(loaded.tasks["step-a"].action_reports.len(), 1);
    }

    #[test]
    fn record_report_unknown_task_returns_error() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path();
        let wf = linear_workflow();

        let state = WorkflowState::new("test", &wf);
        let id = state.workflow_id.clone();
        store::save_state(cwd, &state).unwrap();

        let result = record_report(cwd, &id, "no-such-task", "{}");
        assert!(result.is_err());
    }
}
