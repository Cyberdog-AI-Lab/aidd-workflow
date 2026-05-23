use crate::config::types::{DenyRules, Workflow};
use crate::engine::state::WorkflowState;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct WorkflowOutput {
    pub workflow_id: String,
    pub workflow: String,
    pub status: FlowStatus,
    /// Tasks to execute next; may contain multiple items for concurrent execution.
    pub tasks: Vec<TaskOutput>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowStatus {
    Started,
    InProgress,
    Completed,
    Blocked,
    /// Workflow is paused; call `next` to approve or `reject <task-id>` to retry.
    AwaitingApproval,
}

/// Output for a single executable task returned to the skill layer.
#[derive(Debug, Serialize)]
pub struct TaskOutput {
    pub task_id: String,
    /// Full description of the task (used as display name and for manual tasks).
    pub description: Option<String>,
    /// Prompt for the agent. None for agent-block or manual tasks.
    pub prompt: Option<String>,
    /// Skills to invoke for this task.
    pub skills: Vec<String>,
    /// Custom agent names (from `.claude/agents/`) to spawn in parallel.
    pub agents: Vec<String>,
    /// File path patterns this task may write to.
    pub outputs: Vec<String>,
    /// Deny rules active while this task is InProgress.
    pub deny: Option<DenyRules>,
    /// Whether developer approval is required after this task completes.
    pub approval: bool,
}

#[derive(Debug, Serialize)]
pub struct CompleteOutput {
    pub task_id: String,
    pub allowed: bool,
    pub reason: Option<String>,
    /// Next state when allowed = true.
    pub next: Option<WorkflowOutput>,
}

/// Response to `reject <task-id>`: returns the task to retry.
#[derive(Debug, Serialize)]
pub struct RejectOutput {
    pub task_id: String,
    pub reason: Option<String>,
    /// The task definition for re-dispatch.
    pub task: Option<TaskOutput>,
}

#[derive(Debug, Serialize)]
pub struct StatusOutput {
    pub workflow_id: String,
    pub workflow: String,
    pub started_at: String,
    pub tasks: Vec<TaskStatusItem>,
}

#[derive(Debug, Serialize)]
pub struct TaskStatusItem {
    pub id: String,
    /// Full description shown in status display.
    pub description: String,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct WorkflowListItem {
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    pub task_count: usize,
}

#[derive(Debug, Serialize)]
pub struct ErrorOutput {
    pub error: String,
}

#[derive(Debug, Serialize)]
pub struct ValidateOutput {
    pub valid: bool,
    pub workflow_count: usize,
    pub vars: Vec<String>,
    pub errors: Vec<String>,
}

/// Formats a ValidateOutput as a human-readable multi-line string for terminal display.
pub fn format_validate_text(output: &ValidateOutput) -> String {
    if output.valid {
        format!(
            "config.yml is valid ({} workflow(s), vars: {})",
            output.workflow_count,
            if output.vars.is_empty() {
                "(none)".to_string()
            } else {
                output.vars.join(", ")
            }
        )
    } else {
        let mut lines = format!("config.yml has {} error(s):\n", output.errors.len());
        for (i, e) in output.errors.iter().enumerate() {
            lines.push_str(&format!("  [{}] {}\n", i + 1, e));
        }
        lines.trim_end().to_string()
    }
}

/// Formats a StatusOutput as an ASCII table for terminal display.
pub fn format_status_table(output: &StatusOutput) -> String {
    use comfy_table::{presets::UTF8_BORDERS_ONLY, Table};

    let mut table = Table::new();
    table.load_preset(UTF8_BORDERS_ONLY);
    table.set_header(vec!["TASK ID", "DESCRIPTION", "STATUS"]);

    for task in &output.tasks {
        table.add_row(vec![
            task.id.as_str(),
            task.description.as_str(),
            task.status.as_str(),
        ]);
    }

    format!(
        "Session : {}\nWorkflow: {}\nStarted : {}\n\n{}",
        output.workflow_id, output.workflow, output.started_at, table
    )
}

pub fn build_status(state: &WorkflowState, wf: &Workflow) -> StatusOutput {
    let mut tasks = Vec::new();
    for task in &wf.tasks {
        let status = state
            .tasks
            .get(&task.id)
            .map(|s| format!("{:?}", s.status).to_lowercase())
            .unwrap_or_else(|| "pending".to_string());
        tasks.push(TaskStatusItem {
            id: task.id.clone(),
            description: task.description.clone().unwrap_or_default(),
            status,
        });
        // Sub-agents tracked as "parent_id/agent_name" keys.
        for agent_name in &task.agents {
            let key = format!("{}/{}", task.id, agent_name);
            let sub_status = state
                .tasks
                .get(&key)
                .map(|s| format!("{:?}", s.status).to_lowercase())
                .unwrap_or_else(|| "pending".to_string());
            tasks.push(TaskStatusItem {
                id: key,
                description: format!("agent: {}", agent_name),
                status: sub_status,
            });
        }
    }

    StatusOutput {
        workflow_id: state.workflow_id.clone(),
        workflow: state.workflow.clone(),
        started_at: state.started_at.to_rfc3339(),
        tasks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{Task, Workflow};
    use crate::engine::state::{StepStatus, WorkflowState};

    fn minimal_workflow() -> Workflow {
        Workflow {
            name: "test".to_string(),
            description: None,
            tasks: vec![Task {
                id: "s1".to_string(),
                description: Some("Step 1".to_string()),
                ..Task::default()
            }],
        }
    }

    #[test]
    fn build_status_reflects_task_status() {
        let wf = minimal_workflow();
        let mut state = WorkflowState::new("test", &wf);
        state.tasks.get_mut("s1").unwrap().status = StepStatus::Completed;

        let out = build_status(&state, &wf);
        assert_eq!(out.tasks[0].status, "completed");
    }

    #[test]
    fn build_status_defaults_to_pending() {
        let wf = minimal_workflow();
        let state = WorkflowState::new("test", &wf);

        let out = build_status(&state, &wf);
        assert_eq!(out.tasks[0].status, "pending");
    }

    #[test]
    fn build_status_uses_description_as_display() {
        let wf = minimal_workflow();
        let state = WorkflowState::new("test", &wf);
        let out = build_status(&state, &wf);
        assert_eq!(out.tasks[0].description, "Step 1");
    }

    #[test]
    fn build_status_includes_agent_sub_tasks() {
        let wf = Workflow {
            name: "test".to_string(),
            description: None,
            tasks: vec![Task {
                id: "p".to_string(),
                description: Some("Parallel".to_string()),
                agents: vec!["run-test".to_string(), "run-lint".to_string()],
                ..Task::default()
            }],
        };
        let state = WorkflowState::new("test", &wf);
        let out = build_status(&state, &wf);
        // parent + 2 sub-agents
        assert_eq!(out.tasks.len(), 3);
        assert_eq!(out.tasks[1].id, "p/run-test");
        assert_eq!(out.tasks[1].description, "agent: run-test");
        assert_eq!(out.tasks[2].id, "p/run-lint");
        assert_eq!(out.tasks[2].description, "agent: run-lint");
    }

    #[test]
    fn format_validate_text_valid() {
        let out = ValidateOutput {
            valid: true,
            workflow_count: 2,
            vars: vec!["lint".to_string(), "test".to_string()],
            errors: vec![],
        };
        let text = format_validate_text(&out);
        assert!(text.contains("valid"));
        assert!(text.contains("2 workflow(s)"));
        assert!(text.contains("lint"));
    }

    #[test]
    fn format_validate_text_invalid_shows_all_errors() {
        let out = ValidateOutput {
            valid: false,
            workflow_count: 1,
            vars: vec![],
            errors: vec!["first error".to_string(), "second error".to_string()],
        };
        let text = format_validate_text(&out);
        assert!(text.contains("2 error(s)"));
        assert!(text.contains("[1] first error"));
        assert!(text.contains("[2] second error"));
    }

    #[test]
    fn format_status_table_contains_header_and_rows() {
        let wf = minimal_workflow();
        let state = WorkflowState::new("test", &wf);
        let status = build_status(&state, &wf);
        let table = format_status_table(&status);
        assert!(table.contains("TASK ID"));
        assert!(table.contains("STATUS"));
        assert!(table.contains("s1"));
        assert!(table.contains("pending"));
        assert!(table.contains("Session"));
        assert!(table.contains("Workflow"));
    }
}
