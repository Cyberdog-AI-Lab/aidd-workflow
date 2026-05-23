use crate::config::types::Workflow;
use crate::engine::state::{StepStatus, WorkflowState};

/// Returns IDs of tasks that are currently executable.
/// - Normal tasks (no agents): returned when Pending or InProgress.
/// - Agent tasks: returned only when Pending (agents are spawned once; InProgress means running).
pub fn executable_items(wf: &Workflow, state: &WorkflowState) -> Vec<String> {
    let mut items = Vec::new();

    for task in &wf.tasks {
        let task_state = state.tasks.get(&task.id);
        let task_status = task_state
            .map(|s| &s.status)
            .unwrap_or(&StepStatus::Pending);

        if matches!(task_status, StepStatus::Completed | StepStatus::Failed) {
            continue;
        }

        if !requires_met(wf, state, &task.requires) {
            continue;
        }

        if !task.agents.is_empty() {
            // Agent task: only dispatch when Pending; InProgress means agents are already running.
            if task_status == &StepStatus::Pending {
                items.push(task.id.clone());
            }
        } else if matches!(task_status, StepStatus::Pending | StepStatus::InProgress) {
            items.push(task.id.clone());
        }
    }

    items
}

fn requires_met(wf: &Workflow, state: &WorkflowState, requires: &[String]) -> bool {
    requires.iter().all(|req| {
        state
            .tasks
            .get(req)
            .map(|s| s.status == StepStatus::Completed)
            .unwrap_or(false)
            || wf.tasks.iter().all(|s| s.id != *req) // unknown task: pass through
    })
}

pub fn is_workflow_complete(wf: &Workflow, state: &WorkflowState) -> bool {
    wf.tasks.iter().all(|task| {
        state
            .tasks
            .get(&task.id)
            .map(|s| s.status == StepStatus::Completed)
            .unwrap_or(false)
    })
}

/// Returns the parent task ID if `task_id` is an agent sub-task ("parent/agent").
pub fn parent_of(task_id: &str) -> Option<&str> {
    task_id.find('/').map(|i| &task_id[..i])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{Task, Workflow};
    use crate::engine::state::{StepStatus, WorkflowState};

    fn make_linear_workflow() -> Workflow {
        Workflow {
            name: "test".to_string(),
            description: None,
            tasks: vec![
                Task {
                    id: "a".to_string(),
                    task: Some("A".to_string()),
                    ..Task::default()
                },
                Task {
                    id: "b".to_string(),
                    task: Some("B".to_string()),
                    requires: vec!["a".to_string()],
                    ..Task::default()
                },
            ],
        }
    }

    fn make_workflow_with_agents() -> Workflow {
        Workflow {
            name: "test".to_string(),
            description: None,
            tasks: vec![Task {
                id: "p".to_string(),
                agents: vec!["run-test".to_string(), "run-lint".to_string()],
                ..Task::default()
            }],
        }
    }

    #[test]
    fn first_task_is_executable_with_no_requires() {
        let wf = make_linear_workflow();
        let state = WorkflowState::new("test", &wf);
        let items = executable_items(&wf, &state);
        assert_eq!(items, vec!["a"]);
    }

    #[test]
    fn second_task_blocked_until_first_complete() {
        let wf = make_linear_workflow();
        let mut state = WorkflowState::new("test", &wf);
        state.tasks.get_mut("a").unwrap().status = StepStatus::Completed;
        let items = executable_items(&wf, &state);
        assert_eq!(items, vec!["b"]);
    }

    #[test]
    fn completed_task_not_in_executable_items() {
        let wf = make_linear_workflow();
        let mut state = WorkflowState::new("test", &wf);
        state.tasks.get_mut("a").unwrap().status = StepStatus::Completed;
        state.tasks.get_mut("b").unwrap().status = StepStatus::Completed;
        assert!(executable_items(&wf, &state).is_empty());
    }

    #[test]
    fn agent_task_returned_when_pending() {
        let wf = make_workflow_with_agents();
        let state = WorkflowState::new("test", &wf);
        let items = executable_items(&wf, &state);
        // Returns parent task ID, not individual agent IDs.
        assert_eq!(items, vec!["p"]);
    }

    #[test]
    fn agent_task_not_returned_when_in_progress() {
        let wf = make_workflow_with_agents();
        let mut state = WorkflowState::new("test", &wf);
        state.tasks.get_mut("p").unwrap().status = StepStatus::InProgress;
        let items = executable_items(&wf, &state);
        assert!(
            items.is_empty(),
            "InProgress agent task should not be re-dispatched"
        );
    }

    #[test]
    fn is_workflow_complete_requires_all_tasks() {
        let wf = make_linear_workflow();
        let mut state = WorkflowState::new("test", &wf);
        assert!(!is_workflow_complete(&wf, &state));
        state.tasks.get_mut("a").unwrap().status = StepStatus::Completed;
        assert!(!is_workflow_complete(&wf, &state));
        state.tasks.get_mut("b").unwrap().status = StepStatus::Completed;
        assert!(is_workflow_complete(&wf, &state));
    }

    #[test]
    fn parent_of_returns_parent_for_agent_sub_task() {
        assert_eq!(parent_of("parent/run-test"), Some("parent"));
    }

    #[test]
    fn parent_of_returns_none_for_normal_task() {
        assert_eq!(parent_of("task1"), None);
    }
}
