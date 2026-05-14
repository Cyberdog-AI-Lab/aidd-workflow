use anyhow::Result;
use chrono::Local;
use serde_json::Value;
use std::path::Path;
use crate::config::loader::load_config;
use crate::config::types::Action;
use crate::engine::state::{load_state, save_state, ActionReport, StepStatus};
use crate::engine::gate::hook_check_any_blocked;

/// Claude Code の PostToolUse(Bash) フック
/// テストコマンド実行を検出して checklist.md と state.json に記録する
pub fn handle_post_bash(cwd: &Path, hook_json: &str) -> Result<()> {
    let v: Value = serde_json::from_str(hook_json)?;
    let command = v["tool_input"]["command"].as_str().unwrap_or("");
    let stdout = v["tool_response"]["stdout"].as_str().unwrap_or("");

    let config = match load_config(cwd) {
        Ok(c) => c,
        Err(_) => return Ok(()), // config なし = 対象外プロジェクト
    };
    let test_cmd = config.commands.get("test").map(|s| s.as_str()).unwrap_or("make test");

    if !command.contains(test_cmd) {
        return Ok(());
    }

    // checklist.md に追記
    let checklist_path = cwd.join(".workflow/checklist.md");
    if cwd.join(".workflow").exists() {
        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let entry = format!("## テスト実行: {}\n\n```\n{}\n```\n\n", timestamp, stdout);
        let mut existing = std::fs::read_to_string(&checklist_path).unwrap_or_default();
        existing.push_str(&entry);
        std::fs::write(&checklist_path, existing)?;
    }

    // state.json を更新: in_progress で gate: true アクションを持つステップに記録
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
            parallel.iter().map(|s| {
                (format!("{}/{}", step.id, s.id), s.actions.as_slice())
            }).collect()
        } else {
            vec![(step.id.clone(), step.actions.as_slice())]
        };

        for (sid, actions) in &actions_list {
            let step_state = state.steps.get(sid);
            let is_active = step_state
                .map(|s| s.status == StepStatus::InProgress)
                .unwrap_or(false);
            let has_gate = actions.iter().any(|a| matches!(a, Action::Run { gate: true, .. }));

            if is_active && has_gate {
                let entry = ActionReport {
                    action_index: 0,
                    action_type: "run".to_string(),
                    exit_code: Some(0),
                    stdout: Some(stdout.to_string()),
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

/// Claude Code の PreToolUse(TaskUpdate) フック
/// in_progress ステップに gate 未実行がある場合はブロック JSON を出力する
pub fn handle_pre_taskupdate(cwd: &Path, hook_json: &str) -> Result<Option<String>> {
    let v: Value = serde_json::from_str(hook_json)?;
    let status = v["tool_input"]["status"].as_str().unwrap_or("");
    if status != "completed" {
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

/// Claude Code の PostToolUse(Edit/Write) フック
/// config.yml 編集後にスキーマ検証警告を出力する
pub fn handle_post_edit(cwd: &Path, hook_json: &str) -> Result<Option<String>> {
    let v: Value = serde_json::from_str(hook_json)?;
    let file_path = v["tool_input"]["file_path"].as_str().unwrap_or("");

    if !file_path.ends_with(".workflow/config.yml") {
        return Ok(None);
    }

    match load_config(cwd) {
        Ok(_) => Ok(None),
        Err(e) => Ok(Some(format!(
            "[SCHEMA WARNING] config.yml の検証エラー: {}\n[SCHEMA WARNING] config.yml を確認して自己修正してください",
            e
        ))),
    }
}
