use crate::config::types::Workflow;
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

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
    pub updated_at: Option<DateTime<Utc>>,
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
    pub workflow_id: String,
    pub workflow: String,
    pub started_at: DateTime<Utc>,
    /// Keys: task_id for normal tasks; "parent_id/agent_name" for agent sub-tasks.
    pub tasks: HashMap<String, StepState>,
}

impl WorkflowState {
    pub fn new(workflow_name: &str, wf: &Workflow) -> Self {
        let mut tasks = HashMap::new();
        for task in &wf.tasks {
            tasks.insert(task.id.clone(), StepState::default());
            // Register a state slot for each named agent in the agents list.
            for agent_name in &task.agents {
                tasks.insert(format!("{}/{}", task.id, agent_name), StepState::default());
            }
        }
        WorkflowState {
            workflow_id: Uuid::new_v4().to_string(),
            workflow: workflow_name.to_string(),
            started_at: Utc::now(),
            tasks,
        }
    }

    /// Transitions the parent task to InProgress when at least one agent has started.
    /// Does NOT auto-complete the parent; completion requires an explicit `complete <parent>` call
    /// (which gate-checks that all agents are done).
    pub fn sync_agents_parent(&mut self, parent_id: &str, wf: &Workflow) -> Result<()> {
        let parent_task = match wf.tasks.iter().find(|s| s.id == parent_id) {
            Some(s) => s,
            None => return Ok(()),
        };
        if parent_task.agents.is_empty() {
            return Ok(());
        }

        let any_started = parent_task.agents.iter().any(|agent_name| {
            let key = format!("{}/{}", parent_id, agent_name);
            self.tasks
                .get(&key)
                .map(|s| s.status != StepStatus::Pending)
                .unwrap_or(false)
        });

        if any_started {
            let parent = self.tasks.entry(parent_id.to_string()).or_default();
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
    use crate::config::types::{Task, Workflow};

    fn make_workflow_with_agents() -> Workflow {
        Workflow {
            name: "test".to_string(),
            description: None,
            tasks: vec![Task {
                id: "parent".to_string(),
                agents: vec!["run-test".to_string(), "run-lint".to_string()],
                ..Task::default()
            }],
        }
    }

    #[test]
    fn new_initializes_all_task_keys() {
        let wf = make_workflow_with_agents();
        let state = WorkflowState::new("test", &wf);
        assert!(!state.workflow_id.is_empty());
        assert!(state.tasks.contains_key("parent"));
        assert!(state.tasks.contains_key("parent/run-test"));
        assert!(state.tasks.contains_key("parent/run-lint"));
    }

    #[test]
    fn sync_agents_parent_transitions_to_in_progress_when_any_started() {
        let wf = make_workflow_with_agents();
        let mut state = WorkflowState::new("test", &wf);

        state.tasks.get_mut("parent/run-test").unwrap().status = StepStatus::InProgress;
        state.sync_agents_parent("parent", &wf).unwrap();

        assert_eq!(state.tasks["parent"].status, StepStatus::InProgress);
    }

    #[test]
    fn sync_agents_parent_does_not_auto_complete() {
        let wf = make_workflow_with_agents();
        let mut state = WorkflowState::new("test", &wf);

        // Even when all agents complete, sync does NOT auto-complete the parent.
        state.tasks.get_mut("parent/run-test").unwrap().status = StepStatus::Completed;
        state.tasks.get_mut("parent/run-lint").unwrap().status = StepStatus::Completed;
        state.sync_agents_parent("parent", &wf).unwrap();

        // Parent transitions to InProgress (any_started = true), not Completed.
        assert_eq!(state.tasks["parent"].status, StepStatus::InProgress);
    }

    #[test]
    fn sync_agents_parent_noop_when_all_pending() {
        let wf = make_workflow_with_agents();
        let mut state = WorkflowState::new("test", &wf);
        state.sync_agents_parent("parent", &wf).unwrap();
        assert_eq!(state.tasks["parent"].status, StepStatus::Pending);
    }
}
