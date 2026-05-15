use crate::config::types::Workflow;
use crate::engine::state::{WorkflowState, StepStatus};

/// Returns IDs of items that are currently executable.
/// Normal steps return their step_id; parallel sub-steps return "parent_id/sub_id".
pub fn executable_items(wf: &Workflow, state: &WorkflowState) -> Vec<String> {
    let mut items = Vec::new();

    for step in &wf.steps {
        let step_state = state.steps.get(&step.id);
        let step_status = step_state.map(|s| &s.status).unwrap_or(&StepStatus::Pending);

        if matches!(step_status, StepStatus::Completed | StepStatus::Failed) {
            continue;
        }

        if !requires_met(wf, state, &step.requires) {
            continue;
        }

        if let Some(parallel) = &step.parallel {
            for sub in parallel {
                let key = format!("{}/{}", step.id, sub.id);
                let sub_status = state.steps.get(&key)
                    .map(|s| &s.status)
                    .unwrap_or(&StepStatus::Pending);
                if !matches!(sub_status, StepStatus::Pending | StepStatus::InProgress) {
                    continue;
                }
                let sub_requires_met = sub.requires.iter().all(|req| {
                    let req_key = format!("{}/{}", step.id, req);
                    state.steps.get(&req_key)
                        .map(|s| s.status == StepStatus::Completed)
                        .unwrap_or(false)
                });
                if sub_requires_met {
                    items.push(key);
                }
            }
        } else {
            if matches!(step_status, StepStatus::Pending | StepStatus::InProgress) {
                items.push(step.id.clone());
            }
        }
    }

    items
}

fn requires_met(wf: &Workflow, state: &WorkflowState, requires: &[String]) -> bool {
    requires.iter().all(|req| {
        state.steps.get(req)
            .map(|s| s.status == StepStatus::Completed)
            .unwrap_or(false)
            || wf.steps.iter().all(|s| s.id != *req) // unknown step: pass through
    })
}

pub fn is_workflow_complete(wf: &Workflow, state: &WorkflowState) -> bool {
    wf.steps.iter().all(|step| {
        state.steps.get(&step.id)
            .map(|s| s.status == StepStatus::Completed)
            .unwrap_or(false)
    })
}

/// Returns the parent step ID if `step_id` is a parallel sub-step ("parent/sub").
pub fn parent_of(step_id: &str) -> Option<&str> {
    step_id.find('/').map(|i| &step_id[..i])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{Step, SubStep, Workflow};
    use crate::engine::state::{WorkflowState, StepStatus};

    fn make_linear_workflow() -> Workflow {
        Workflow {
            name: "test".to_string(),
            description: None,
            steps: vec![
                Step {
                    id: "a".to_string(),
                    name: "A".to_string(),
                    description: None,
                    actions: vec![],
                    parallel: None,
                    checklist_key: None,
                    requires: vec![],
                },
                Step {
                    id: "b".to_string(),
                    name: "B".to_string(),
                    description: None,
                    actions: vec![],
                    parallel: None,
                    checklist_key: None,
                    requires: vec!["a".to_string()],
                },
            ],
        }
    }

    fn make_parallel_workflow() -> Workflow {
        Workflow {
            name: "test".to_string(),
            description: None,
            steps: vec![
                Step {
                    id: "p".to_string(),
                    name: "Parallel".to_string(),
                    description: None,
                    actions: vec![],
                    parallel: Some(vec![
                        SubStep { id: "x".to_string(), name: None, description: None, actions: vec![], requires: vec![] },
                        SubStep { id: "y".to_string(), name: None, description: None, actions: vec![], requires: vec![] },
                    ]),
                    checklist_key: None,
                    requires: vec![],
                },
            ],
        }
    }

    #[test]
    fn first_step_is_executable_with_no_requires() {
        let wf = make_linear_workflow();
        let state = WorkflowState::new("test", &wf);
        let items = executable_items(&wf, &state);
        assert_eq!(items, vec!["a"]);
    }

    #[test]
    fn second_step_blocked_until_first_complete() {
        let wf = make_linear_workflow();
        let mut state = WorkflowState::new("test", &wf);
        state.steps.get_mut("a").unwrap().status = StepStatus::Completed;
        let items = executable_items(&wf, &state);
        assert_eq!(items, vec!["b"]);
    }

    #[test]
    fn completed_step_not_in_executable_items() {
        let wf = make_linear_workflow();
        let mut state = WorkflowState::new("test", &wf);
        state.steps.get_mut("a").unwrap().status = StepStatus::Completed;
        state.steps.get_mut("b").unwrap().status = StepStatus::Completed;
        assert!(executable_items(&wf, &state).is_empty());
    }

    #[test]
    fn parallel_sub_steps_both_returned() {
        let wf = make_parallel_workflow();
        let state = WorkflowState::new("test", &wf);
        let items = executable_items(&wf, &state);
        assert!(items.contains(&"p/x".to_string()));
        assert!(items.contains(&"p/y".to_string()));
    }

    #[test]
    fn is_workflow_complete_requires_all_steps() {
        let wf = make_linear_workflow();
        let mut state = WorkflowState::new("test", &wf);
        assert!(!is_workflow_complete(&wf, &state));
        state.steps.get_mut("a").unwrap().status = StepStatus::Completed;
        assert!(!is_workflow_complete(&wf, &state));
        state.steps.get_mut("b").unwrap().status = StepStatus::Completed;
        assert!(is_workflow_complete(&wf, &state));
    }

    #[test]
    fn parent_of_returns_parent_for_sub_step() {
        assert_eq!(parent_of("parent/sub"), Some("parent"));
    }

    #[test]
    fn parent_of_returns_none_for_normal_step() {
        assert_eq!(parent_of("step1"), None);
    }

    fn make_parallel_workflow_with_sub_requires() -> Workflow {
        Workflow {
            name: "test".to_string(),
            description: None,
            steps: vec![
                Step {
                    id: "p".to_string(),
                    name: "Parallel".to_string(),
                    description: None,
                    actions: vec![],
                    parallel: Some(vec![
                        SubStep { id: "x".to_string(), name: None, description: None, actions: vec![], requires: vec![] },
                        SubStep { id: "y".to_string(), name: None, description: None, actions: vec![], requires: vec!["x".to_string()] },
                    ]),
                    checklist_key: None,
                    requires: vec![],
                },
            ],
        }
    }

    #[test]
    fn sub_step_with_unmet_requires_not_executable() {
        let wf = make_parallel_workflow_with_sub_requires();
        let state = WorkflowState::new("test", &wf);
        let items = executable_items(&wf, &state);
        // Only x is executable; y requires x which is not complete
        assert_eq!(items, vec!["p/x"]);
        assert!(!items.contains(&"p/y".to_string()));
    }

    #[test]
    fn sub_step_becomes_executable_after_requires_complete() {
        let wf = make_parallel_workflow_with_sub_requires();
        let mut state = WorkflowState::new("test", &wf);
        state.steps.get_mut("p/x").unwrap().status = StepStatus::Completed;
        let items = executable_items(&wf, &state);
        assert_eq!(items, vec!["p/y"]);
    }
}
