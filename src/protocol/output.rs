use crate::config::types::Workflow;
use crate::engine::state::WorkflowState;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Serialize)]
pub struct WorkflowOutput {
    pub session_id: String,
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
    pub step_id: String,
    pub action_index: usize,
    /// Display name of the step.
    pub step_name: String,
    /// When true, this item is part of a parallel block and may run concurrently
    /// with other parallel=true items in the same response.
    pub parallel: bool,
    #[serde(flatten)]
    pub action: ResolvedAction,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResolvedAction {
    Run {
        command: String,
        gate: bool,
    },
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
    /// Step with no actions and no parallel block; Claude works from the description.
    Manual {
        description: String,
        checklist_key: Option<String>,
    },
}

#[derive(Debug, Serialize)]
pub struct CompleteOutput {
    pub step_id: String,
    pub allowed: bool,
    pub reason: Option<String>,
    /// Next actions when allowed = true.
    pub next: Option<WorkflowOutput>,
}

#[derive(Debug, Serialize)]
pub struct StatusOutput {
    pub session_id: String,
    pub workflow: String,
    pub started_at: String,
    pub steps: Vec<StepStatusItem>,
}

#[derive(Debug, Serialize)]
pub struct StepStatusItem {
    pub id: String,
    pub name: String,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct WorkflowListItem {
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    pub step_count: usize,
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
    table.set_header(vec!["STEP ID", "NAME", "STATUS"]);

    for step in &output.steps {
        table.add_row(vec![
            step.id.as_str(),
            step.name.as_str(),
            step.status.as_str(),
        ]);
    }

    format!(
        "Session : {}\nWorkflow: {}\nStarted : {}\n\n{}",
        output.session_id, output.workflow, output.started_at, table
    )
}

pub fn build_status(state: &WorkflowState, wf: &Workflow) -> StatusOutput {
    let mut steps = Vec::new();
    for step in &wf.steps {
        let status = state
            .steps
            .get(&step.id)
            .map(|s| format!("{:?}", s.status).to_lowercase())
            .unwrap_or_else(|| "pending".to_string());
        steps.push(StepStatusItem {
            id: step.id.clone(),
            name: step.name.clone(),
            status,
        });
        if let Some(parallel) = &step.parallel {
            for sub in parallel {
                let key = format!("{}/{}", step.id, sub.id);
                let sub_status = state
                    .steps
                    .get(&key)
                    .map(|s| format!("{:?}", s.status).to_lowercase())
                    .unwrap_or_else(|| "pending".to_string());
                let sub_name = sub.name.as_deref().unwrap_or(&sub.id).to_string();
                steps.push(StepStatusItem {
                    id: key,
                    name: sub_name,
                    status: sub_status,
                });
            }
        }
    }

    StatusOutput {
        session_id: state.session_id.clone(),
        workflow: state.workflow.clone(),
        started_at: state.started_at.to_rfc3339(),
        steps,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{Step, Workflow};
    use crate::engine::state::{StepStatus, WorkflowState};

    fn minimal_workflow() -> Workflow {
        Workflow {
            name: "test".to_string(),
            description: None,
            steps: vec![Step {
                id: "s1".to_string(),
                name: "S1".to_string(),
                description: None,
                actions: vec![],
                parallel: None,
                checklist_key: None,
                requires: vec![],
            }],
        }
    }

    #[test]
    fn build_status_reflects_step_status() {
        let wf = minimal_workflow();
        let mut state = WorkflowState::new("test", &wf);
        state.steps.get_mut("s1").unwrap().status = StepStatus::Completed;

        let out = build_status(&state, &wf);
        assert_eq!(out.steps[0].status, "completed");
    }

    #[test]
    fn build_status_defaults_to_pending() {
        let wf = minimal_workflow();
        let state = WorkflowState::new("test", &wf);

        let out = build_status(&state, &wf);
        assert_eq!(out.steps[0].status, "pending");
    }

    #[test]
    fn build_status_includes_parallel_sub_steps() {
        use crate::config::types::SubStep;
        let wf = Workflow {
            name: "test".to_string(),
            description: None,
            steps: vec![Step {
                id: "p".to_string(),
                name: "Parallel".to_string(),
                description: None,
                actions: vec![],
                parallel: Some(vec![
                    SubStep {
                        id: "a".to_string(),
                        name: Some("A".to_string()),
                        description: None,
                        actions: vec![],
                        requires: vec![],
                    },
                    SubStep {
                        id: "b".to_string(),
                        name: None,
                        description: None,
                        actions: vec![],
                        requires: vec![],
                    },
                ]),
                checklist_key: None,
                requires: vec![],
            }],
        };
        let state = WorkflowState::new("test", &wf);
        let out = build_status(&state, &wf);
        // parent + 2 sub-steps
        assert_eq!(out.steps.len(), 3);
        assert_eq!(out.steps[1].id, "p/a");
        assert_eq!(out.steps[1].name, "A");
        assert_eq!(out.steps[2].id, "p/b");
        assert_eq!(out.steps[2].name, "b");
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
        assert!(table.contains("STEP ID"));
        assert!(table.contains("STATUS"));
        assert!(table.contains("s1"));
        assert!(table.contains("pending"));
        assert!(table.contains("Session"));
        assert!(table.contains("Workflow"));
    }
}
