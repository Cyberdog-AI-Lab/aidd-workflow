use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use anyhow::Result;
use chrono::{DateTime, Utc};
use uuid::Uuid;
use crate::config::types::Workflow;

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
    Failed,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct StepState {
    #[serde(default)]
    pub status: StepStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    /// Whether a gate:true run action has been executed and recorded.
    #[serde(default)]
    pub gate_recorded: bool,
    #[serde(default)]
    pub action_reports: Vec<ActionReport>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ActionReport {
    pub action_index: usize,
    pub action_type: String,
    pub exit_code: Option<i32>,
    pub stdout: Option<String>,
    pub recorded_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct WorkflowState {
    pub session_id: String,
    pub workflow: String,
    pub started_at: DateTime<Utc>,
    /// Keys: step_id for normal steps; "parent_id/sub_id" for parallel sub-steps.
    pub steps: HashMap<String, StepState>,
}

impl WorkflowState {
    pub fn new(workflow_name: &str, wf: &Workflow) -> Self {
        let mut steps = HashMap::new();
        for step in &wf.steps {
            steps.insert(step.id.clone(), StepState::default());
            if let Some(parallel) = &step.parallel {
                for sub in parallel {
                    steps.insert(format!("{}/{}", step.id, sub.id), StepState::default());
                }
            }
        }
        WorkflowState {
            session_id: Uuid::new_v4().to_string(),
            workflow: workflow_name.to_string(),
            started_at: Utc::now(),
            steps,
        }
    }

    /// Derives parent step status from sub-step statuses and updates it in place.
    pub fn sync_parallel_parent(&mut self, parent_id: &str, wf: &Workflow) -> Result<()> {
        let parent_step = match wf.steps.iter().find(|s| s.id == parent_id) {
            Some(s) => s,
            None => return Ok(()),
        };
        let parallel = match &parent_step.parallel {
            Some(p) => p,
            None => return Ok(()),
        };

        let all_completed = parallel.iter().all(|sub| {
            let key = format!("{}/{}", parent_id, sub.id);
            self.steps.get(&key).map(|s| s.status == StepStatus::Completed).unwrap_or(false)
        });
        let any_started = parallel.iter().any(|sub| {
            let key = format!("{}/{}", parent_id, sub.id);
            self.steps.get(&key)
                .map(|s| s.status != StepStatus::Pending)
                .unwrap_or(false)
        });

        let parent = self.steps.entry(parent_id.to_string()).or_default();
        if all_completed {
            parent.status = StepStatus::Completed;
            if parent.completed_at.is_none() {
                parent.completed_at = Some(Utc::now());
            }
        } else if any_started {
            if parent.status == StepStatus::Pending {
                parent.status = StepStatus::InProgress;
                parent.started_at = Some(Utc::now());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{Step, SubStep, Workflow};

    fn make_workflow_with_parallel() -> Workflow {
        Workflow {
            name: "test".to_string(),
            description: None,
            steps: vec![
                Step {
                    id: "parent".to_string(),
                    name: "Parent".to_string(),
                    description: None,
                    actions: vec![],
                    parallel: Some(vec![
                        SubStep { id: "a".to_string(), name: None, description: None, actions: vec![] },
                        SubStep { id: "b".to_string(), name: None, description: None, actions: vec![] },
                    ]),
                    checklist_key: None,
                    requires: vec![],
                },
            ],
        }
    }

    #[test]
    fn new_initializes_all_step_keys() {
        let wf = make_workflow_with_parallel();
        let state = WorkflowState::new("test", &wf);
        assert!(state.steps.contains_key("parent"));
        assert!(state.steps.contains_key("parent/a"));
        assert!(state.steps.contains_key("parent/b"));
    }

    #[test]
    fn sync_parallel_parent_completes_when_all_subs_complete() {
        let wf = make_workflow_with_parallel();
        let mut state = WorkflowState::new("test", &wf);

        state.steps.get_mut("parent/a").unwrap().status = StepStatus::Completed;
        state.steps.get_mut("parent/b").unwrap().status = StepStatus::Completed;
        state.sync_parallel_parent("parent", &wf).unwrap();

        assert_eq!(state.steps["parent"].status, StepStatus::Completed);
    }

    #[test]
    fn sync_parallel_parent_in_progress_when_partial() {
        let wf = make_workflow_with_parallel();
        let mut state = WorkflowState::new("test", &wf);

        state.steps.get_mut("parent/a").unwrap().status = StepStatus::InProgress;
        state.sync_parallel_parent("parent", &wf).unwrap();

        assert_eq!(state.steps["parent"].status, StepStatus::InProgress);
    }
}
