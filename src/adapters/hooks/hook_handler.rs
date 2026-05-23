use crate::config::loader::{load_config, matches_pattern};
use crate::engine::state::StepStatus;
use crate::engine::store::load_state;
use crate::providers::claude_code::hook_parser::{PostEditEvent, PreBashEvent, PreEditEvent};
use anyhow::Result;
use std::path::Path;

/// Claude Code PostToolUse(Bash) hook. No-op.
pub fn handle_post_bash(_cwd: &Path, _hook_json: &str) -> Result<()> {
    Ok(())
}

/// Claude Code PostToolUse(Edit/Write) hook.
/// Outputs a schema warning when config.yml fails validation after an edit.
pub fn handle_post_edit(cwd: &Path, hook_json: &str) -> Result<Option<String>> {
    let event: PostEditEvent = match serde_json::from_str(hook_json) {
        Ok(e) => e,
        Err(_) => return Ok(None),
    };

    if !event.tool_input.file_path.ends_with(".workflow/config.yml") {
        return Ok(None);
    }

    match load_config(cwd) {
        Ok(_) => Ok(None),
        Err(e) => Ok(Some(format!(
            "[SCHEMA WARNING] config.yml validation error: {}\n[SCHEMA WARNING] Please review and fix config.yml",
            e
        ))),
    }
}

/// Claude Code PreToolUse(Edit/Write) hook.
/// Returns `{"decision":"ask"}` when the file is outside allow_files for the active task,
/// or `{"decision":"block"}` when it matches deny.files.
pub fn handle_pre_edit(cwd: &Path, hook_json: &str) -> Result<Option<String>> {
    let event: PreEditEvent = match serde_json::from_str(hook_json) {
        Ok(e) => e,
        Err(_) => return Ok(None),
    };

    let config = match load_config(cwd) {
        Ok(c) => c,
        Err(_) => return Ok(None),
    };
    let state = match load_state(cwd)? {
        Some(s) => s,
        None => return Ok(None),
    };
    let wf = match config.workflows.get(&state.workflow) {
        Some(w) => w,
        None => return Ok(None),
    };

    let abs_path = &event.tool_input.file_path;
    let rel_path = std::path::Path::new(abs_path)
        .strip_prefix(cwd)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| abs_path.clone());

    for task in &wf.tasks {
        let is_active = state
            .tasks
            .get(&task.id)
            .map(|s| s.status == StepStatus::InProgress)
            .unwrap_or(false);

        if !is_active {
            continue;
        }

        // allow_files: non-empty list means the file must match at least one pattern.
        if !task.allow_files.is_empty() {
            let allowed = task
                .allow_files
                .iter()
                .any(|p| matches_pattern(p, &rel_path));
            if !allowed {
                let decision = serde_json::json!({
                    "decision": "ask",
                    "reason": format!(
                        "[{}] '{}' is outside allow_files for task '{}'",
                        task.id, rel_path, task.name
                    )
                });
                return Ok(Some(decision.to_string()));
            }
        }

        // deny.files: any matching pattern is an explicit block.
        if let Some(deny) = &task.deny {
            for pattern in &deny.files {
                if matches_pattern(pattern, &rel_path) {
                    let decision = serde_json::json!({
                        "decision": "block",
                        "reason": format!(
                            "[{}] editing '{}' is denied by task '{}' (rule: '{}')",
                            task.id, rel_path, task.name, pattern
                        )
                    });
                    return Ok(Some(decision.to_string()));
                }
            }
        }
    }

    Ok(None)
}

/// Claude Code PreToolUse(Bash) hook.
/// Returns `{"decision":"block"}` when the command matches deny.commands for the active task.
pub fn handle_pre_bash(cwd: &Path, hook_json: &str) -> Result<Option<String>> {
    let event: PreBashEvent = match serde_json::from_str(hook_json) {
        Ok(e) => e,
        Err(_) => return Ok(None),
    };

    let config = match load_config(cwd) {
        Ok(c) => c,
        Err(_) => return Ok(None),
    };
    let state = match load_state(cwd)? {
        Some(s) => s,
        None => return Ok(None),
    };
    let wf = match config.workflows.get(&state.workflow) {
        Some(w) => w,
        None => return Ok(None),
    };

    let command = &event.tool_input.command;

    for task in &wf.tasks {
        let is_active = state
            .tasks
            .get(&task.id)
            .map(|s| s.status == StepStatus::InProgress)
            .unwrap_or(false);

        if !is_active {
            continue;
        }

        if let Some(deny) = &task.deny {
            for pattern in &deny.commands {
                if matches_command_pattern(pattern, command) {
                    let decision = serde_json::json!({
                        "decision": "block",
                        "reason": format!(
                            "[{}] command '{}' is denied by task '{}' (rule: '{}')",
                            task.id, command, task.name, pattern
                        )
                    });
                    return Ok(Some(decision.to_string()));
                }
            }
        }
    }

    Ok(None)
}

/// Matches a Bash command string against a deny pattern.
/// /regex/ patterns use full-text regex; plain strings use substring (partial) match.
fn matches_command_pattern(pattern: &str, command: &str) -> bool {
    if let Some(inner) = pattern.strip_prefix('/').and_then(|s| s.strip_suffix('/')) {
        regex::Regex::new(inner)
            .map(|re| re.is_match(command))
            .unwrap_or(false)
    } else {
        command.contains(pattern)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{DenyRules, Task, Workflow};
    use crate::engine::state::{StepStatus, WorkflowState};
    use crate::engine::store::save_state;
    use std::collections::HashMap;
    use tempfile::tempdir;

    fn setup_workflow_dir(dir: &std::path::Path) {
        let wf_dir = dir.join(".workflow");
        std::fs::create_dir_all(&wf_dir).unwrap();
    }

    fn make_workflow_with_task(task: Task) -> Workflow {
        Workflow {
            name: "test".to_string(),
            description: None,
            tasks: vec![task],
        }
    }

    fn active_state(workflow_name: &str, task_id: &str, wf: &Workflow) -> WorkflowState {
        let mut state = WorkflowState::new(workflow_name, wf);
        state.tasks.get_mut(task_id).unwrap().status = StepStatus::InProgress;
        state
    }

    fn write_config(dir: &std::path::Path, yaml: &str) {
        std::fs::write(dir.join(".workflow/config.yml"), yaml).unwrap();
    }

    // --- matches_command_pattern ---

    #[test]
    fn command_pattern_plain_partial_match() {
        assert!(matches_command_pattern("git push", "git push origin main"));
        assert!(matches_command_pattern("git push", "git push"));
        assert!(!matches_command_pattern("git push", "git pull"));
    }

    #[test]
    fn command_pattern_regex() {
        assert!(matches_command_pattern("/^git push/", "git push origin"));
        assert!(!matches_command_pattern("/^git push/", "echo git push"));
    }

    // --- handle_pre_edit ---

    #[test]
    fn pre_edit_allows_when_no_workflow_active() {
        let dir = tempdir().unwrap();
        setup_workflow_dir(dir.path());
        let json = serde_json::json!({
            "cwd": dir.path().to_str().unwrap(),
            "tool_input": {"file_path": dir.path().join("src/main.rs").to_str().unwrap()}
        });
        let result = handle_pre_edit(dir.path(), &json.to_string()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn pre_edit_blocks_deny_file() {
        let dir = tempdir().unwrap();
        setup_workflow_dir(dir.path());

        let task = Task {
            id: "impl".to_string(),
            name: "Implement".to_string(),
            deny: Some(DenyRules {
                files: vec!["docs/specs/**".to_string()],
                commands: vec![],
            }),
            ..Task::default()
        };
        let wf = make_workflow_with_task(task);
        let mut state = active_state("test", "impl", &wf);
        state.workflow = "test".to_string();

        // Write minimal config so load_config succeeds.
        write_config(
            dir.path(),
            "commands:\n  test: make test\nworkflows:\n  test:\n    name: test\n    tasks:\n      - id: impl\n        name: Implement\n        deny:\n          files:\n            - \"docs/specs/**\"\n",
        );
        save_state(dir.path(), &state).unwrap();

        let file_path = dir.path().join("docs/specs/design.md");
        let json = serde_json::json!({
            "cwd": dir.path().to_str().unwrap(),
            "tool_input": {"file_path": file_path.to_str().unwrap()}
        });
        let result = handle_pre_edit(dir.path(), &json.to_string()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(v["decision"].as_str().unwrap(), "block");
    }

    #[test]
    fn pre_edit_asks_when_outside_allow_files() {
        let dir = tempdir().unwrap();
        setup_workflow_dir(dir.path());

        write_config(
            dir.path(),
            "commands:\n  test: make test\nworkflows:\n  test:\n    name: test\n    tasks:\n      - id: impl\n        name: Implement\n        allow_files:\n          - \"src/**\"\n",
        );

        let wf_for_state = Workflow {
            name: "test".to_string(),
            description: None,
            tasks: vec![Task {
                id: "impl".to_string(),
                name: "Implement".to_string(),
                allow_files: vec!["src/**".to_string()],
                ..Task::default()
            }],
        };
        let state = active_state("test", "impl", &wf_for_state);
        save_state(dir.path(), &state).unwrap();

        let file_path = dir.path().join("docs/README.md");
        let json = serde_json::json!({
            "cwd": dir.path().to_str().unwrap(),
            "tool_input": {"file_path": file_path.to_str().unwrap()}
        });
        let result = handle_pre_edit(dir.path(), &json.to_string()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(v["decision"].as_str().unwrap(), "ask");
    }

    #[test]
    fn pre_edit_allows_matching_allow_file() {
        let dir = tempdir().unwrap();
        setup_workflow_dir(dir.path());

        write_config(
            dir.path(),
            "commands:\n  test: make test\nworkflows:\n  test:\n    name: test\n    tasks:\n      - id: impl\n        name: Implement\n        allow_files:\n          - \"src/**\"\n",
        );

        let wf_for_state = Workflow {
            name: "test".to_string(),
            description: None,
            tasks: vec![Task {
                id: "impl".to_string(),
                name: "Implement".to_string(),
                allow_files: vec!["src/**".to_string()],
                ..Task::default()
            }],
        };
        let state = active_state("test", "impl", &wf_for_state);
        save_state(dir.path(), &state).unwrap();

        let file_path = dir.path().join("src/main.rs");
        let json = serde_json::json!({
            "cwd": dir.path().to_str().unwrap(),
            "tool_input": {"file_path": file_path.to_str().unwrap()}
        });
        let result = handle_pre_edit(dir.path(), &json.to_string()).unwrap();
        assert!(result.is_none(), "expected allow but got: {:?}", result);
    }

    // --- handle_pre_bash ---

    #[test]
    fn pre_bash_blocks_denied_command() {
        let dir = tempdir().unwrap();
        setup_workflow_dir(dir.path());

        write_config(
            dir.path(),
            "commands:\n  test: make test\nworkflows:\n  test:\n    name: test\n    tasks:\n      - id: impl\n        name: Implement\n        deny:\n          commands:\n            - \"git push\"\n",
        );

        let wf_for_state = Workflow {
            name: "test".to_string(),
            description: None,
            tasks: vec![Task {
                id: "impl".to_string(),
                name: "Implement".to_string(),
                deny: Some(DenyRules {
                    files: vec![],
                    commands: vec!["git push".to_string()],
                }),
                ..Task::default()
            }],
        };
        let state = active_state("test", "impl", &wf_for_state);
        save_state(dir.path(), &state).unwrap();

        let json = serde_json::json!({
            "cwd": dir.path().to_str().unwrap(),
            "tool_input": {"command": "git push origin main"}
        });
        let result = handle_pre_bash(dir.path(), &json.to_string()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(v["decision"].as_str().unwrap(), "block");
    }

    #[test]
    fn pre_bash_allows_non_denied_command() {
        let dir = tempdir().unwrap();
        setup_workflow_dir(dir.path());

        write_config(
            dir.path(),
            "commands:\n  test: make test\nworkflows:\n  test:\n    name: test\n    tasks:\n      - id: impl\n        name: Implement\n        deny:\n          commands:\n            - \"git push\"\n",
        );

        let wf_for_state = Workflow {
            name: "test".to_string(),
            description: None,
            tasks: vec![Task {
                id: "impl".to_string(),
                name: "Implement".to_string(),
                deny: Some(DenyRules {
                    files: vec![],
                    commands: vec!["git push".to_string()],
                }),
                ..Task::default()
            }],
        };
        let state = active_state("test", "impl", &wf_for_state);
        save_state(dir.path(), &state).unwrap();

        let json = serde_json::json!({
            "cwd": dir.path().to_str().unwrap(),
            "tool_input": {"command": "cargo test"}
        });
        let result = handle_pre_bash(dir.path(), &json.to_string()).unwrap();
        assert!(result.is_none(), "expected allow but got: {:?}", result);
    }

    // Suppress unused import warning for HashMap in this test module.
    fn _use_hashmap(_: HashMap<(), ()>) {}
}
