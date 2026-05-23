use crate::config::types::Workflow;
use crate::engine::state::{StepStatus, WorkflowState};

pub struct GateResult {
    pub allowed: bool,
    pub reason: Option<String>,
}

/// Checks whether the given task can transition to Completed.
///
/// For parent tasks: verifies all `requires` are Completed AND all `agents` are Completed.
/// For agent sub-tasks ("parent/agent"): skips requires check (sub-agents have no requires).
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

    // Sub-agent completions skip all gate checks (no requires, no nested agents).
    if sub_id.is_some() {
        return GateResult {
            allowed: true,
            reason: None,
        };
    }

    // Check requires: all dependent tasks must be Completed.
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

    // Check agents: all named agents must be Completed before the parent can complete.
    for agent_name in &task.agents {
        let key = format!("{}/{}", cfg_task_id, agent_name);
        let met = state
            .tasks
            .get(&key)
            .map(|s| s.status == StepStatus::Completed)
            .unwrap_or(false);
        if !met {
            return GateResult {
                allowed: false,
                reason: Some(format!(
                    "task '{}' cannot complete until agent '{}' is done",
                    task_id, agent_name
                )),
            };
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
                    description: Some("A".to_string()),
                    ..Task::default()
                },
                Task {
                    id: "b".to_string(),
                    description: Some("B".to_string()),
                    requires: vec!["a".to_string()],
                    ..Task::default()
                },
            ],
        }
    }

    fn workflow_with_agents() -> Workflow {
        Workflow {
            name: "test".to_string(),
            description: None,
            tasks: vec![Task {
                id: "parallel".to_string(),
                agents: vec!["run-test".to_string(), "run-lint".to_string()],
                ..Task::default()
            }],
        }
    }

    #[test]
    fn gate_passes_when_no_requires_and_no_agents() {
        let wf = workflow_with_requires();
        let state = WorkflowState::new("test", &wf);
        let result = check(&wf, &state, "a");
        assert!(result.allowed);
    }

    #[test]
    fn gate_blocks_when_requires_not_complete() {
        let wf = workflow_with_requires();
        let state = WorkflowState::new("test", &wf);
        let result = check(&wf, &state, "b");
        assert!(!result.allowed);
        assert!(result.reason.unwrap().contains("'a'"));
    }

    #[test]
    fn gate_passes_when_requires_complete() {
        let wf = workflow_with_requires();
        let mut state = WorkflowState::new("test", &wf);
        state.tasks.get_mut("a").unwrap().status = StepStatus::Completed;
        let result = check(&wf, &state, "b");
        assert!(result.allowed);
    }

    #[test]
    fn gate_blocks_parent_when_agent_not_complete() {
        let wf = workflow_with_agents();
        let state = WorkflowState::new("test", &wf);
        let result = check(&wf, &state, "parallel");
        assert!(!result.allowed);
        assert!(result.reason.unwrap().contains("run-test"));
    }

    #[test]
    fn gate_passes_parent_when_all_agents_complete() {
        let wf = workflow_with_agents();
        let mut state = WorkflowState::new("test", &wf);
        state.tasks.get_mut("parallel/run-test").unwrap().status = StepStatus::Completed;
        state.tasks.get_mut("parallel/run-lint").unwrap().status = StepStatus::Completed;
        let result = check(&wf, &state, "parallel");
        assert!(result.allowed);
    }

    #[test]
    fn gate_always_passes_for_agent_sub_task() {
        let wf = workflow_with_agents();
        let state = WorkflowState::new("test", &wf);
        // Sub-agent completes without any gate check.
        let result = check(&wf, &state, "parallel/run-test");
        assert!(result.allowed);
    }
}
