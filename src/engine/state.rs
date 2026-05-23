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
    /// Keys: task_id for normal tasks; "parent_id/sub_id" for agent sub-tasks.
    pub tasks: HashMap<String, StepState>,
}

impl WorkflowState {
    pub fn new(workflow_name: &str, wf: &Workflow) -> Self {
        let mut tasks = HashMap::new();
        for task in &wf.tasks {
            tasks.insert(task.id.clone(), StepState::default());
            if let Some(agents) = &task.agents {
                for sub in agents {
                    tasks.insert(format!("{}/{}", task.id, sub.id), StepState::default());
                }
            }
        }
        WorkflowState {
            workflow_id: Uuid::new_v4().to_string(),
            workflow: workflow_name.to_string(),
            started_at: Utc::now(),
            tasks,
        }
    }

    /// Derives parent task status from sub-agent statuses and updates it in place.
    pub fn sync_agents_parent(&mut self, parent_id: &str, wf: &Workflow) -> Result<()> {
        let parent_task = match wf.tasks.iter().find(|s| s.id == parent_id) {
            Some(s) => s,
            None => return Ok(()),
        };
        let agents = match &parent_task.agents {
            Some(p) => p,
            None => return Ok(()),
        };

        let all_completed = agents.iter().all(|sub| {
            let key = format!("{}/{}", parent_id, sub.id);
            self.tasks
                .get(&key)
                .map(|s| s.status == StepStatus::Completed)
                .unwrap_or(false)
        });
        let any_started = agents.iter().any(|sub| {
            let key = format!("{}/{}", parent_id, sub.id);
            self.tasks
                .get(&key)
                .map(|s| s.status != StepStatus::Pending)
                .unwrap_or(false)
        });

        let parent = self.tasks.entry(parent_id.to_string()).or_default();
        if all_completed {
            parent.status = StepStatus::Completed;
            if parent.completed_at.is_none() {
                parent.completed_at = Some(Utc::now());
            }
        } else if any_started && parent.status == StepStatus::Pending {
            parent.status = StepStatus::InProgress;
            parent.started_at = Some(Utc::now());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{SubAgentTask, Task, Workflow};

    fn make_workflow_with_agents() -> Workflow {
        Workflow {
            name: "test".to_string(),
            description: None,
            tasks: vec![Task {
                id: "parent".to_string(),
                name: "Parent".to_string(),
                agents: Some(vec![
                    SubAgentTask {
                        id: "a".to_string(),
                        name: None,
                        description: None,
                        actions: vec![],
                        requires: vec![],
                    },
                    SubAgentTask {
                        id: "b".to_string(),
                        name: None,
                        description: None,
                        actions: vec![],
                        requires: vec![],
                    },
                ]),
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
        assert!(state.tasks.contains_key("parent/a"));
        assert!(state.tasks.contains_key("parent/b"));
    }

    #[test]
    fn sync_agents_parent_completes_when_all_subs_complete() {
        let wf = make_workflow_with_agents();
        let mut state = WorkflowState::new("test", &wf);

        state.tasks.get_mut("parent/a").unwrap().status = StepStatus::Completed;
        state.tasks.get_mut("parent/b").unwrap().status = StepStatus::Completed;
        state.sync_agents_parent("parent", &wf).unwrap();

        assert_eq!(state.tasks["parent"].status, StepStatus::Completed);
    }

    #[test]
    fn sync_agents_parent_in_progress_when_partial() {
        let wf = make_workflow_with_agents();
        let mut state = WorkflowState::new("test", &wf);

        state.tasks.get_mut("parent/a").unwrap().status = StepStatus::InProgress;
        state.sync_agents_parent("parent", &wf).unwrap();

        assert_eq!(state.tasks["parent"].status, StepStatus::InProgress);
    }
}
