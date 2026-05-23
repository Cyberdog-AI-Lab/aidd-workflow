use crate::config::loader::matches_pattern;
use crate::config::types::Workflow;
use crate::engine::state::{StepStatus, WorkflowState};
use std::path::Path;

pub struct GateResult {
    pub allowed: bool,
    pub reason: Option<String>,
}

/// Checks whether the given step can transition to Completed.
pub fn check(wf: &Workflow, state: &WorkflowState, step_id: &str, cwd: &Path) -> GateResult {
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

        // guards check: referenced step must be completed and required_files must exist
        for guard in &step.guards {
            let guard_done = state
                .steps
                .get(&guard.step)
                .map(|s| s.status == StepStatus::Completed)
                .unwrap_or(false);
            if !guard_done {
                return GateResult {
                    allowed: false,
                    reason: Some(format!(
                        "step '{}' guard requires '{}' to be completed first",
                        step_id, guard.step
                    )),
                };
            }
            for pattern in &guard.required_files {
                if !any_file_matches(cwd, pattern) {
                    return GateResult {
                        allowed: false,
                        reason: Some(format!(
                            "step '{}' guard: no file matching '{}' found (required by step '{}')",
                            step_id, pattern, guard.step
                        )),
                    };
                }
            }
        }
    }

    GateResult {
        allowed: true,
        reason: None,
    }
}

/// Returns true if any file under `cwd` matches the given pattern.
fn any_file_matches(cwd: &Path, pattern: &str) -> bool {
    walk_files(cwd, cwd, pattern)
}

fn walk_files(root: &Path, dir: &Path, pattern: &str) -> bool {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return false,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if walk_files(root, &path, pattern) {
                return true;
            }
        } else if let Ok(rel) = path.strip_prefix(root) {
            let rel_str = rel.to_string_lossy();
            if matches_pattern(pattern, &rel_str) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{Guard, Step, Workflow};
    use crate::engine::state::{StepStatus, WorkflowState};
    use std::path::PathBuf;

    fn dummy_cwd() -> PathBuf {
        std::env::temp_dir()
    }

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

        let result = check(&wf, &state, "b", &dummy_cwd());
        assert!(result.allowed);
    }

    #[test]
    fn requires_check_blocks_when_dep_not_complete() {
        let wf = workflow_with_requires();
        let state = WorkflowState::new("test", &wf);

        let result = check(&wf, &state, "b", &dummy_cwd());
        assert!(!result.allowed);
    }

    #[test]
    fn requires_check_passes_when_dep_complete() {
        let wf = workflow_with_requires();
        let mut state = WorkflowState::new("test", &wf);
        state.steps.get_mut("a").unwrap().status = StepStatus::Completed;

        let result = check(&wf, &state, "b", &dummy_cwd());
        assert!(result.allowed);
    }

    #[test]
    fn guards_check_blocks_when_guard_step_not_done() {
        let wf = Workflow {
            name: "test".to_string(),
            description: None,
            steps: vec![
                Step {
                    id: "design".to_string(),
                    name: "Design".to_string(),
                    ..Step::default()
                },
                Step {
                    id: "impl".to_string(),
                    name: "Implement".to_string(),
                    guards: vec![Guard {
                        step: "design".to_string(),
                        required_files: vec![],
                    }],
                    ..Step::default()
                },
            ],
        };
        let state = WorkflowState::new("test", &wf);

        let result = check(&wf, &state, "impl", &dummy_cwd());
        assert!(!result.allowed);
        assert!(result.reason.unwrap().contains("guard"));
    }
}
