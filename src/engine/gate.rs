use crate::config::types::{Workflow, Action};
use crate::engine::state::{WorkflowState, StepStatus};

pub struct GateResult {
    pub allowed: bool,
    pub reason: Option<String>,
}

/// Checks whether the given step can transition to Completed.
pub fn check(wf: &Workflow, state: &WorkflowState, step_id: &str) -> GateResult {
    let (cfg_step_id, sub_id) = if let Some(idx) = step_id.find('/') {
        (&step_id[..idx], Some(&step_id[idx + 1..]))
    } else {
        (step_id, None)
    };

    let step = match wf.steps.iter().find(|s| s.id == cfg_step_id) {
        Some(s) => s,
        None => return GateResult {
            allowed: false,
            reason: Some(format!("step '{}' not found in config.yml", step_id)),
        },
    };

    // requires check (parallel sub-steps do not have their own requires)
    if sub_id.is_none() {
        for req in &step.requires {
            let met = state.steps.get(req)
                .map(|s| s.status == StepStatus::Completed)
                .unwrap_or(false);
            if !met {
                return GateResult {
                    allowed: false,
                    reason: Some(format!(
                        "step '{}' cannot complete until '{}' is done",
                        step_id, req
                    )),
                };
            }
        }
    }

    let actions: &[Action] = if let Some(sub) = sub_id {
        let parallel = step.parallel.as_deref().unwrap_or(&[]);
        match parallel.iter().find(|s| s.id == sub) {
            Some(s) => &s.actions,
            None => return GateResult {
                allowed: false,
                reason: Some(format!("sub-step '{}' not found", step_id)),
            },
        }
    } else {
        &step.actions
    };

    let has_gate = actions.iter().any(|a| matches!(a, Action::Run { gate: true, .. }));
    if has_gate {
        let recorded = state.steps.get(step_id)
            .map(|s| s.gate_recorded)
            .unwrap_or(false);
        if !recorded {
            return GateResult {
                allowed: false,
                reason: Some(format!(
                    "gate check failed: gate action for step '{}' has not been executed yet",
                    step_id
                )),
            };
        }
    }

    GateResult { allowed: true, reason: None }
}

/// Used by hooks: returns a block reason if any in-progress step has an unrecorded gate action.
pub fn hook_check_any_blocked(wf: &Workflow, state: &WorkflowState) -> Option<String> {
    for step in &wf.steps {
        let actions_to_check: Vec<&[Action]> = if let Some(parallel) = &step.parallel {
            parallel.iter().map(|s| s.actions.as_slice()).collect()
        } else {
            vec![step.actions.as_slice()]
        };

        let step_ids: Vec<String> = if step.parallel.is_some() {
            step.parallel.as_ref().unwrap().iter()
                .map(|s| format!("{}/{}", step.id, s.id))
                .collect()
        } else {
            vec![step.id.clone()]
        };

        for (actions, sid) in actions_to_check.iter().zip(step_ids.iter()) {
            let has_gate = actions.iter().any(|a| matches!(a, Action::Run { gate: true, .. }));
            if !has_gate {
                continue;
            }
            let step_state = state.steps.get(sid);
            let is_active = step_state
                .map(|s| s.status == StepStatus::InProgress)
                .unwrap_or(false);
            let recorded = step_state.map(|s| s.gate_recorded).unwrap_or(false);

            if is_active && !recorded {
                return Some(format!(
                    "gate check failed: gate action for step '{}' has not been executed",
                    sid
                ));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{Action, Step, Workflow};
    use crate::engine::state::{WorkflowState, StepStatus};

    fn workflow_with_gate() -> Workflow {
        Workflow {
            name: "test".to_string(),
            description: None,
            steps: vec![
                Step {
                    id: "test".to_string(),
                    name: "Test".to_string(),
                    description: None,
                    actions: vec![Action::Run {
                        command: "make test".to_string(),
                        gate: true,
                    }],
                    parallel: None,
                    checklist_key: None,
                    requires: vec![],
                },
            ],
        }
    }

    fn workflow_with_requires() -> Workflow {
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

    #[test]
    fn gate_check_blocks_when_not_recorded() {
        let wf = workflow_with_gate();
        let mut state = WorkflowState::new("test", &wf);
        state.steps.get_mut("test").unwrap().status = StepStatus::InProgress;

        let result = check(&wf, &state, "test");
        assert!(!result.allowed);
        assert!(result.reason.unwrap().contains("gate check failed"));
    }

    #[test]
    fn gate_check_passes_when_recorded() {
        let wf = workflow_with_gate();
        let mut state = WorkflowState::new("test", &wf);
        let s = state.steps.get_mut("test").unwrap();
        s.status = StepStatus::InProgress;
        s.gate_recorded = true;

        let result = check(&wf, &state, "test");
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
        state.steps.get_mut("a").unwrap().status = StepStatus::Completed;

        let result = check(&wf, &state, "b");
        assert!(result.allowed);
    }

    #[test]
    fn hook_check_blocks_in_progress_gate_step() {
        let wf = workflow_with_gate();
        let mut state = WorkflowState::new("test", &wf);
        state.steps.get_mut("test").unwrap().status = StepStatus::InProgress;

        let reason = hook_check_any_blocked(&wf, &state);
        assert!(reason.is_some());
    }

    #[test]
    fn hook_check_passes_when_gate_recorded() {
        let wf = workflow_with_gate();
        let mut state = WorkflowState::new("test", &wf);
        let s = state.steps.get_mut("test").unwrap();
        s.status = StepStatus::InProgress;
        s.gate_recorded = true;

        let reason = hook_check_any_blocked(&wf, &state);
        assert!(reason.is_none());
    }
}
