use crate::config::types::Workflow;
use crate::engine::state::{StepStatus, WorkflowState};

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
        None => {
            return GateResult {
                allowed: false,
                reason: Some(format!("step '{}' not found in config.yml", step_id)),
            }
        }
    };

    // requires check (parallel sub-steps do not have their own requires)
    if sub_id.is_none() {
        for req in &step.requires {
            let met = state
                .steps
                .get(req)
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

    GateResult {
        allowed: true,
        reason: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{Step, Workflow};
    use crate::engine::state::{StepStatus, WorkflowState};

    fn workflow_with_requires() -> Workflow {
        Workflow {
            name: "test".to_string(),
            description: None,
            steps: vec![
                Step {
                    id: "a".to_string(),
                    name: "A".to_string(),
                    ..Step::default()
                },
                Step {
                    id: "b".to_string(),
                    name: "B".to_string(),
                    requires: vec!["a".to_string()],
                    ..Step::default()
                },
            ],
        }
    }

    #[test]
    fn gate_check_passes_without_unmet_requires() {
        let wf = workflow_with_requires();
        let mut state = WorkflowState::new("test", &wf);
        state.steps.get_mut("a").unwrap().status = StepStatus::Completed;

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
        state.steps.get_mut("a").unwrap().status = StepStatus::Completed;

        let result = check(&wf, &state, "b");
        assert!(result.allowed);
    }
}
