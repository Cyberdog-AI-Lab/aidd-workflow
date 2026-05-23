use crate::config::types::Workflow;
use crate::engine::state::{StepStatus, WorkflowState};

pub struct GateResult {
    pub allowed: bool,
    pub reason: Option<String>,
}

/// Checks whether the given task can transition to Completed.
pub fn check(wf: &Workflow, state: &WorkflowState, task_id: &str) -> GateResult {
    let (cfg_task_id, sub_id) = if let Some(idx) = task_id.find('/') {
        (&task_id[..idx], Some(&task_id[idx + 1..]))
    } else {
        (task_id, None)
    };

    let task = match wf.tasks.iter().find(|s| s.id == cfg_task_id) {
        Some(s) => s,
        None => {
            return GateResult {
                allowed: false,
                reason: Some(format!("task '{}' not found in config.yml", task_id)),
            }
        }
    };

    // requires check (agent sub-tasks do not have their own requires)
    if sub_id.is_none() {
        for req in &task.requires {
            let met = state
                .tasks
                .get(req)
                .map(|s| s.status == StepStatus::Completed)
                .unwrap_or(false);
            if !met {
                return GateResult {
                    allowed: false,
                    reason: Some(format!(
                        "task '{}' cannot complete until '{}' is done",
                        task_id, req
                    )),
                };
            }
        }
    }

    GateResult {
        allowed: true,
        reason: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{Task, Workflow};
    use crate::engine::state::{StepStatus, WorkflowState};

    fn workflow_with_requires() -> Workflow {
        Workflow {
            name: "test".to_string(),
            description: None,
            tasks: vec![
                Task {
                    id: "a".to_string(),
                    name: "A".to_string(),
                    ..Task::default()
                },
                Task {
                    id: "b".to_string(),
                    name: "B".to_string(),
                    requires: vec!["a".to_string()],
                    ..Task::default()
                },
            ],
        }
    }

    #[test]
    fn gate_check_passes_without_unmet_requires() {
        let wf = workflow_with_requires();
        let mut state = WorkflowState::new("test", &wf);
        state.tasks.get_mut("a").unwrap().status = StepStatus::Completed;

        let result = check(&wf, &state, "b");
        assert!(result.allowed);
    }

    #[test]
    fn requires_check_blocks_when_dep_not_complete() {
        let wf = workflow_with_requires();
        let state = WorkflowState::new("test", &wf);

        let result = check(&wf, &state, "b");
        assert!(!result.allowed);
    }

    #[test]
    fn requires_check_passes_when_dep_complete() {
        let wf = workflow_with_requires();
        let mut state = WorkflowState::new("test", &wf);
        state.tasks.get_mut("a").unwrap().status = StepStatus::Completed;

        let result = check(&wf, &state, "b");
        assert!(result.allowed);
    }
}
