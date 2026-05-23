use crate::config::types::Workflow;
use crate::engine::state::WorkflowState;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Serialize)]
pub struct WorkflowOutput {
    pub workflow_id: String,
    pub workflow: String,
    pub status: FlowStatus,
    /// Actions to execute next; may contain multiple items for concurrent execution.
    pub actions: Vec<ActionItem>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowStatus {
    Started,
    InProgress,
    Completed,
    Blocked,
}

#[derive(Debug, Serialize)]
pub struct ActionItem {
    pub task_id: String,
    pub action_index: usize,
    /// Display name of the task.
    pub task_name: String,
    /// When true, this item is part of an agents block and must be run as a sub-agent.
    pub sub_agent: bool,
    #[serde(flatten)]
    pub action: ResolvedAction,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResolvedAction {
    Agent {
        prompt: String,
        background: bool,
    },
    Skill {
        skill: String,
        args: Vec<String>,
    },
    Workflow {
        workflow: String,
        inputs: HashMap<String, String>,
    },
    /// Task with no actions and no agents block; Claude works from the description.
    Manual {
        description: String,
    },
}

#[derive(Debug, Serialize)]
pub struct CompleteOutput {
    pub task_id: String,
    pub allowed: bool,
    pub reason: Option<String>,
    /// Next actions when allowed = true.
    pub next: Option<WorkflowOutput>,
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
    pub name: String,
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
    pub commands: Vec<String>,
    pub errors: Vec<String>,
}

/// Formats a ValidateOutput as a human-readable multi-line string for terminal display.
pub fn format_validate_text(output: &ValidateOutput) -> String {
    if output.valid {
        format!(
            "config.yml is valid ({} workflow(s), commands: {})",
            output.workflow_count,
            if output.commands.is_empty() {
                "(none)".to_string()
            } else {
                output.commands.join(", ")
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
    table.set_header(vec!["TASK ID", "NAME", "STATUS"]);

    for task in &output.tasks {
        table.add_row(vec![
            task.id.as_str(),
            task.name.as_str(),
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
            name: task.name.clone(),
            status,
        });
        if let Some(agents) = &task.agents {
            for sub in agents {
                let key = format!("{}/{}", task.id, sub.id);
                let sub_status = state
                    .tasks
                    .get(&key)
                    .map(|s| format!("{:?}", s.status).to_lowercase())
                    .unwrap_or_else(|| "pending".to_string());
                let sub_name = sub.name.as_deref().unwrap_or(&sub.id).to_string();
                tasks.push(TaskStatusItem {
                    id: key,
                    name: sub_name,
                    status: sub_status,
                });
            }
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
                name: "S1".to_string(),
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
    fn build_status_includes_agent_sub_tasks() {
        use crate::config::types::SubagentTask;
        let wf = Workflow {
            name: "test".to_string(),
            description: None,
            tasks: vec![Task {
                id: "p".to_string(),
                name: "Parallel".to_string(),
                agents: Some(vec![
                    SubagentTask {
                        id: "a".to_string(),
                        name: Some("A".to_string()),
                        description: None,
                        actions: vec![],
                        requires: vec![],
                    },
                    SubagentTask {
                        id: "b".to_string(),
                        name: None,
                        description: None,
                        actions: vec![],
                        requires: vec![],
                    },
                ]),
                ..Task::default()
            }],
        };
        let state = WorkflowState::new("test", &wf);
        let out = build_status(&state, &wf);
        // parent + 2 sub-agents
        assert_eq!(out.tasks.len(), 3);
        assert_eq!(out.tasks[1].id, "p/a");
        assert_eq!(out.tasks[1].name, "A");
        assert_eq!(out.tasks[2].id, "p/b");
        assert_eq!(out.tasks[2].name, "b");
    }

    #[test]
    fn format_validate_text_valid() {
        let out = ValidateOutput {
            valid: true,
            workflow_count: 2,
            commands: vec!["lint".to_string(), "test".to_string()],
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
            commands: vec![],
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
