use crate::config::loader::load_config;
use crate::engine::gate::hook_check_any_blocked;
use crate::engine::store::load_state;
use crate::providers::claude_code::hook_parser::{PostEditEvent, PreTaskUpdateEvent};
use anyhow::Result;
use std::path::Path;

/// Claude Code PostToolUse(Bash) hook.
/// No-op in Phase 2+: gate_recorded is now set by `complete` via post_commands execution.
pub fn handle_post_bash(_cwd: &Path, _hook_json: &str) -> Result<()> {
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
