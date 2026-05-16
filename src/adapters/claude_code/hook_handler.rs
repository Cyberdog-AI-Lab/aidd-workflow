use crate::config::loader::load_config;
use crate::config::types::Action;
use crate::engine::gate::hook_check_any_blocked;
use crate::engine::state::{ActionReport, StepStatus};
use crate::engine::store::{load_state, save_state};
use crate::providers::claude_code::hook_parser::{
    PostBashEvent, PostEditEvent, PreTaskUpdateEvent,
};
use anyhow::Result;
use std::path::Path;

/// Claude Code PostToolUse(Bash) hook.
/// Detects test command execution and records it in state (SQLite).
pub fn handle_post_bash(cwd: &Path, hook_json: &str) -> Result<()> {
    let event: PostBashEvent = match serde_json::from_str(hook_json) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    let command = &event.tool_input.command;
    let stdout = &event.tool_response.stdout;

    let config = match load_config(cwd) {
        Ok(c) => c,
        Err(_) => return Ok(()),
    };
    let test_cmd = config
        .commands
        .get("test")
        .map(|s| s.as_str())
        .unwrap_or("make test");

    if !command.contains(test_cmd) {
        return Ok(());
    }

    let mut state = match load_state(cwd)? {
        Some(s) => s,
        None => return Ok(()),
    };
    let wf = match config.workflows.get(&state.workflow) {
        Some(w) => w,
        None => return Ok(()),
    };

    let mut updated = false;
    for step in &wf.steps {
        let actions_list: Vec<(String, &[Action])> = if let Some(parallel) = &step.parallel {
            parallel
                .iter()
                .map(|s| (format!("{}/{}", step.id, s.id), s.actions.as_slice()))
                .collect()
        } else {
            vec![(step.id.clone(), step.actions.as_slice())]
        };

        for (sid, actions) in &actions_list {
            let step_state = state.steps.get(sid);
            let is_active = step_state
                .map(|s| s.status == StepStatus::InProgress)
                .unwrap_or(false);
            let has_gate = actions
                .iter()
                .any(|a| matches!(a, Action::Run { gate: true, .. }));

            if is_active && has_gate {
                let entry = ActionReport {
                    action_index: 0,
                    action_type: "run".to_string(),
                    exit_code: Some(0),
                    stdout: Some(stdout.clone()),
                    recorded_at: chrono::Utc::now(),
                };
                let s = state.steps.entry(sid.clone()).or_default();
                s.gate_recorded = true;
                s.action_reports.push(entry);
                updated = true;
            }
        }
    }

    if updated {
        save_state(cwd, &state)?;
    }
    Ok(())
}

/// Claude Code PreToolUse(TaskUpdate) hook.
/// Outputs a block JSON if any in-progress step has an unrecorded gate action.
pub fn handle_pre_taskupdate(cwd: &Path, hook_json: &str) -> Result<Option<String>> {
    let event: PreTaskUpdateEvent = match serde_json::from_str(hook_json) {
        Ok(e) => e,
        Err(_) => return Ok(None),
    };

    if event.tool_input.status != "completed" {
        return Ok(None);
    }

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

    if let Some(reason) = hook_check_any_blocked(wf, &state) {
        let decision = serde_json::json!({
            "decision": "block",
            "reason": reason
        });
        return Ok(Some(decision.to_string()));
    }
    Ok(None)
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
