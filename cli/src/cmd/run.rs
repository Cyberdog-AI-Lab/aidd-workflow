use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{
    extract::{Path as AxumPath, State},
    routing::post,
    Router,
};
use chrono::Utc;
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::config::loader::load_config;
use crate::config::types::{Config, Workflow};
use crate::engine::state::{ActionReport, StepStatus};
use crate::engine::{dag, executor, gate, store};
use crate::protocol::input::ReportInput;
use crate::protocol::output::{FlowStatus, TaskOutput};

enum RunEvent {
    Complete {
        task_id: String,
    },
    Report {
        task_id: String,
        body: String,
    },
    Next,
    Reject {
        task_id: String,
        reason: Option<String>,
    },
}

struct AppState {
    tx: mpsc::Sender<RunEvent>,
}

#[derive(Deserialize)]
struct RejectBody {
    reason: Option<String>,
}

async fn handle_complete(
    AxumPath(task_id): AxumPath<String>,
    State(state): State<Arc<AppState>>,
) -> &'static str {
    state.tx.send(RunEvent::Complete { task_id }).await.ok();
    "ok"
}

async fn handle_report(
    AxumPath(task_id): AxumPath<String>,
    State(state): State<Arc<AppState>>,
    body: String,
) -> &'static str {
    state.tx.send(RunEvent::Report { task_id, body }).await.ok();
    "ok"
}

async fn handle_next(State(state): State<Arc<AppState>>) -> &'static str {
    state.tx.send(RunEvent::Next).await.ok();
    "ok"
}

async fn handle_reject(
    AxumPath(task_id): AxumPath<String>,
    State(state): State<Arc<AppState>>,
    body: String,
) -> &'static str {
    let reason = serde_json::from_str::<RejectBody>(&body)
        .ok()
        .and_then(|b| b.reason);
    state
        .tx
        .send(RunEvent::Reject { task_id, reason })
        .await
        .ok();
    "ok"
}

pub async fn run_workflow(
    cwd: PathBuf,
    workflow_name: String,
    callback_port: u16,
    callback_url: String,
    webhook_url: String,
) -> Result<()> {
    let (tx, mut rx) = mpsc::channel::<RunEvent>(32);

    let app_state = Arc::new(AppState { tx });
    let app = Router::new()
        .route("/complete/:task_id", post(handle_complete))
        .route("/report/:task_id", post(handle_report))
        .route("/next", post(handle_next))
        .route("/reject/:task_id", post(handle_reject))
        .with_state(app_state);

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", callback_port))
        .await
        .context("failed to bind callback server")?;
    eprintln!(
        "[run] callback server listening on 127.0.0.1:{}",
        callback_port
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let config = load_config(&cwd)?;
    let wf = config
        .workflows
        .get(&workflow_name)
        .with_context(|| format!("workflow '{}' not found in config.yml", workflow_name))?
        .clone();

    let state = crate::engine::state::WorkflowState::new(&workflow_name, &wf);
    let workflow_id = state.workflow_id.clone();
    store::save_state(&cwd, &state)?;
    eprintln!(
        "[run] workflow '{}' started (id: {})",
        workflow_name, workflow_id
    );

    let mut dispatched: HashSet<String> = HashSet::new();

    let initial = executor::build_next(&wf, &state, &config);
    dispatch_tasks(
        &initial.tasks,
        &callback_url,
        &webhook_url,
        &workflow_id,
        &mut dispatched,
    )
    .await?;

    loop {
        match rx.recv().await {
            None => break,

            Some(RunEvent::Report { task_id, body }) => {
                if let Err(e) = record_report(&cwd, &workflow_id, &task_id, &body) {
                    eprintln!("[run] report error for '{}': {}", task_id, e);
                }
            }

            Some(RunEvent::Complete { task_id }) => {
                dispatched.remove(&task_id);
                eprintln!("[run] task '{}' completed", task_id);

                let next_tasks = complete_task(&cwd, &workflow_id, &task_id, &wf, &config)?;

                // Workflow finished or awaiting approval when next_tasks is None
                if let Some(tasks) = next_tasks {
                    dispatch_tasks(
                        &tasks,
                        &callback_url,
                        &webhook_url,
                        &workflow_id,
                        &mut dispatched,
                    )
                    .await?;
                }

                // Check if the workflow record was cleared (completed) or paused
                match store::load_state_by_id(&cwd, &workflow_id)? {
                    None => {
                        eprintln!("[run] workflow '{}' completed", workflow_name);
                        break;
                    }
                    Some(_) => {
                        let wf_status = store::get_workflow_status(&cwd, &workflow_id)?;
                        if wf_status == "awaiting_approval" {
                            eprintln!(
                                "[run] paused awaiting approval for task '{}'; \
                                 POST /next to approve or /reject/{} to retry",
                                task_id, task_id
                            );
                        }
                    }
                }
            }

            Some(RunEvent::Next) => {
                let wf_status = store::get_workflow_status(&cwd, &workflow_id)?;
                if wf_status != "awaiting_approval" {
                    eprintln!(
                        "[run] /next called but workflow is not awaiting approval (status: {})",
                        wf_status
                    );
                    continue;
                }
                store::set_workflow_status(&cwd, &workflow_id, "active")?;
                eprintln!("[run] approved; dispatching next tasks");

                let state = store::load_state_by_id(&cwd, &workflow_id)?
                    .context("workflow state not found after approval")?;
                let next = executor::build_next(&wf, &state, &config);
                if matches!(next.status, FlowStatus::Completed) {
                    store::clear_state_by_id(&cwd, &workflow_id)?;
                    eprintln!(
                        "[run] workflow '{}' completed after approval",
                        workflow_name
                    );
                    break;
                }
                dispatch_tasks(
                    &next.tasks,
                    &callback_url,
                    &webhook_url,
                    &workflow_id,
                    &mut dispatched,
                )
                .await?;
            }

            Some(RunEvent::Reject { task_id, reason }) => {
                let wf_status = store::get_workflow_status(&cwd, &workflow_id)?;
                if wf_status != "awaiting_approval" {
                    eprintln!("[run] /reject called but workflow is not awaiting approval");
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
                eprintln!("[run] task '{}' rejected; re-dispatching", task_id);

                let task_output = build_task_output(&task_id, &wf, &config);
                if let Some(t) = task_output {
                    dispatched.remove(&task_id);
                    dispatch_tasks(
                        &[t],
                        &callback_url,
                        &webhook_url,
                        &workflow_id,
                        &mut dispatched,
                    )
                    .await?;
                }
            }
        }
    }

    Ok(())
}

/// Marks a task as completed, enforces the gate check, handles the approval flag, and
/// returns the next set of tasks to dispatch.  Returns None when the workflow has
/// finished (cleared from the store) or has paused waiting for approval.
pub fn complete_task(
    cwd: &Path,
    workflow_id: &str,
    task_id: &str,
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
pub fn record_report(cwd: &Path, workflow_id: &str, task_id: &str, body: &str) -> Result<()> {
    let mut state =
        store::load_state_by_id(cwd, workflow_id)?.context("workflow state not found")?;

    if !state.tasks.contains_key(task_id) {
        anyhow::bail!("unknown task_id '{}' in workflow", task_id);
    }

    let s = state.tasks.entry(task_id.to_string()).or_default();
    if s.status == StepStatus::Pending {
        s.status = StepStatus::InProgress;
        s.started_at = Some(Utc::now());
    }
    if let Ok(input) = serde_json::from_str::<ReportInput>(body) {
        s.action_reports.push(ActionReport {
            action_index: input.action_index,
            action_type: input.action_type,
            exit_code: input.exit_code,
            stdout: input.stdout,
            recorded_at: Utc::now(),
        });
    }

    store::save_state(cwd, &state)?;
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
        let payload = serde_json::json!({
            "task_id": task.task_id,
            "task": task.task,
            "prompt": task.prompt,
            "callback_url": callback_url,
            "workflow_id": workflow_id,
            "outputs": task.outputs,
            "deny": task.deny,
        });
        eprintln!("[run] dispatching task '{}' to webhook", task.task_id);
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

        let next = complete_task(cwd, &id, "step-a", &wf, &config)
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

        let result = complete_task(cwd, &id, "step-b", &wf, &config).unwrap();
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

        let result = complete_task(cwd, &id, "review", &wf, &config).unwrap();
        assert!(result.is_none());
        assert_eq!(
            store::get_workflow_status(cwd, &id).unwrap(),
            "awaiting_approval"
        );
    }

    #[test]
    fn record_report_transitions_to_in_progress() {
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
        assert_eq!(loaded.tasks["step-a"].status, StepStatus::InProgress);
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
