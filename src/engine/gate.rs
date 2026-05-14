use crate::config::types::{Workflow, Action};
use crate::engine::state::{WorkflowState, StepStatus};

pub struct GateResult {
    pub allowed: bool,
    pub reason: Option<String>,
}

/// step_id のステップを completed に遷移させてよいか確認する
pub fn check(wf: &Workflow, state: &WorkflowState, step_id: &str) -> GateResult {
    // サブステップの場合は親ステップのコンフィグを探す
    let (cfg_step_id, sub_id) = if let Some(idx) = step_id.find('/') {
        (&step_id[..idx], Some(&step_id[idx + 1..]))
    } else {
        (step_id, None)
    };

    let step = match wf.steps.iter().find(|s| s.id == cfg_step_id) {
        Some(s) => s,
        None => return GateResult {
            allowed: false,
            reason: Some(format!("ステップ '{}' が config.yml に見つかりません", step_id)),
        },
    };

    // requires チェック（並列サブステップは requires を持たない）
    if sub_id.is_none() {
        for req in &step.requires {
            let met = state.steps.get(req)
                .map(|s| s.status == StepStatus::Completed)
                .unwrap_or(false);
            if !met {
                return GateResult {
                    allowed: false,
                    reason: Some(format!(
                        "ステップ '{}' は '{}' が完了してから実行できます",
                        step_id, req
                    )),
                };
            }
        }
    }

    // gate アクションのチェック
    let actions: &[Action] = if let Some(sub) = sub_id {
        let parallel = step.parallel.as_deref().unwrap_or(&[]);
        match parallel.iter().find(|s| s.id == sub) {
            Some(s) => &s.actions,
            None => return GateResult {
                allowed: false,
                reason: Some(format!("サブステップ '{}' が見つかりません", step_id)),
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
                    "gate チェック失敗: ステップ '{}' の gate アクションが未実行です。先にコマンドを実行してください",
                    step_id
                )),
            };
        }
    }

    GateResult { allowed: true, reason: None }
}

/// フック経由でのゲートチェック（in_progress の全ステップを確認）
/// ブロックすべき理由があれば Some(reason) を返す
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
                    "gate チェック失敗: ステップ '{}' の gate アクションが実行されていません。先にコマンドを実行してください",
                    sid
                ));
            }
        }
    }
    None
}
