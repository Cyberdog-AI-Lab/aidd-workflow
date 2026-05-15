use serde::Serialize;
use std::collections::HashMap;
use crate::engine::state::WorkflowState;
use crate::config::types::Workflow;

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

pub fn build_status(state: &WorkflowState, wf: &Workflow) -> StatusOutput {
    let mut steps = Vec::new();
    for step in &wf.steps {
        let status = state.steps.get(&step.id)
            .map(|s| format!("{:?}", s.status).to_lowercase())
            .unwrap_or_else(|| "pending".to_string());
        steps.push(StepStatusItem { id: step.id.clone(), name: step.name.clone(), status });
        if let Some(parallel) = &step.parallel {
            for sub in parallel {
                let key = format!("{}/{}", step.id, sub.id);
                let sub_status = state.steps.get(&key)
                    .map(|s| format!("{:?}", s.status).to_lowercase())
                    .unwrap_or_else(|| "pending".to_string());
                let sub_name = sub.name.as_deref().unwrap_or(&sub.id).to_string();
                steps.push(StepStatusItem { id: key, name: sub_name, status: sub_status });
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
    use crate::engine::state::{WorkflowState, StepStatus};

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
        use crate::config::types::{SubStep};
        let wf = Workflow {
            name: "test".to_string(),
            description: None,
            steps: vec![Step {
                id: "p".to_string(),
                name: "Parallel".to_string(),
                description: None,
                actions: vec![],
                parallel: Some(vec![
                    SubStep { id: "a".to_string(), name: Some("A".to_string()), description: None, actions: vec![], requires: vec![] },
                    SubStep { id: "b".to_string(), name: None, description: None, actions: vec![], requires: vec![] },
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
}
