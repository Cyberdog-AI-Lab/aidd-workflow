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

    // post_commands gate: requires gate_recorded to be true
    if sub_id.is_none() && !step.post_commands.is_empty() {
        let recorded = state
            .steps
            .get(step_id)
            .map(|s| s.gate_recorded)
            .unwrap_or(false);
        if !recorded {
            return GateResult {
                allowed: false,
                reason: Some(format!(
                    "gate check failed: post_commands for step '{}' have not been executed yet",
                    step_id
                )),
            };
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

/// Used by hooks: returns a block reason if any in-progress step has unexecuted post_commands.
pub fn hook_check_any_blocked(wf: &Workflow, state: &WorkflowState) -> Option<String> {
    for step in &wf.steps {
        if step.post_commands.is_empty() {
            continue;
        }

        let step_state = state.steps.get(&step.id);
        let is_active = step_state
            .map(|s| s.status == StepStatus::InProgress)
            .unwrap_or(false);
        let recorded = step_state.map(|s| s.gate_recorded).unwrap_or(false);

        if is_active && !recorded {
            return Some(format!(
                "gate check failed: post_commands for step '{}' have not been executed",
                step.id
            ));
        }
    }
    None
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

    fn workflow_with_post_commands() -> Workflow {
        Workflow {
            name: "test".to_string(),
            description: None,
            steps: vec![Step {
                id: "test".to_string(),
                name: "Test".to_string(),
                description: None,
                actions: vec![],
                parallel: None,
                requires: vec![],
                pre_commands: vec![],
                post_commands: vec!["make test".to_string()],
                allow_files: vec![],
                deny: None,
                guards: vec![],
            }],
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
                    requires: vec![],
                    pre_commands: vec![],
                    post_commands: vec![],
                    allow_files: vec![],
                    deny: None,
                    guards: vec![],
                },
                Step {
                    id: "b".to_string(),
                    name: "B".to_string(),
                    description: None,
                    actions: vec![],
                    parallel: None,
                    requires: vec!["a".to_string()],
                    pre_commands: vec![],
                    post_commands: vec![],
                    allow_files: vec![],
                    deny: None,
                    guards: vec![],
                },
            ],
        }
    }

    #[test]
    fn gate_check_blocks_when_post_commands_not_recorded() {
        let wf = workflow_with_post_commands();
        let mut state = WorkflowState::new("test", &wf);
        state.steps.get_mut("test").unwrap().status = StepStatus::InProgress;

        let result = check(&wf, &state, "test", &dummy_cwd());
        assert!(!result.allowed);
        assert!(result.reason.unwrap().contains("gate check failed"));
    }

    #[test]
    fn gate_check_passes_when_post_commands_recorded() {
        let wf = workflow_with_post_commands();
        let mut state = WorkflowState::new("test", &wf);
        let s = state.steps.get_mut("test").unwrap();
        s.status = StepStatus::InProgress;
        s.gate_recorded = true;

        let result = check(&wf, &state, "test", &dummy_cwd());
        assert!(result.allowed);
    }

    #[test]
    fn gate_check_passes_without_post_commands() {
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
                    description: None,
                    actions: vec![],
                    parallel: None,
                    requires: vec![],
                    pre_commands: vec![],
                    post_commands: vec![],
                    allow_files: vec![],
                    deny: None,
                    guards: vec![],
                },
                Step {
                    id: "impl".to_string(),
                    name: "Implement".to_string(),
                    description: None,
                    actions: vec![],
                    parallel: None,
                    requires: vec![],
                    pre_commands: vec![],
                    post_commands: vec![],
                    allow_files: vec![],
                    deny: None,
                    guards: vec![Guard {
                        step: "design".to_string(),
                        required_files: vec![],
                    }],
                },
            ],
        };
        let state = WorkflowState::new("test", &wf);

        let result = check(&wf, &state, "impl", &dummy_cwd());
        assert!(!result.allowed);
        assert!(result.reason.unwrap().contains("guard"));
    }

    #[test]
    fn hook_check_blocks_in_progress_post_commands_step() {
        let wf = workflow_with_post_commands();
        let mut state = WorkflowState::new("test", &wf);
        state.steps.get_mut("test").unwrap().status = StepStatus::InProgress;

        let reason = hook_check_any_blocked(&wf, &state);
        assert!(reason.is_some());
    }

    #[test]
    fn hook_check_passes_when_gate_recorded() {
        let wf = workflow_with_post_commands();
        let mut state = WorkflowState::new("test", &wf);
        let s = state.steps.get_mut("test").unwrap();
        s.status = StepStatus::InProgress;
        s.gate_recorded = true;

        let reason = hook_check_any_blocked(&wf, &state);
        assert!(reason.is_none());
    }
}
