use crate::config::types::Workflow;
use crate::engine::state::{WorkflowState, StepStatus};

/// 現在の state から実行可能なアイテムの ID を返す。
/// - 通常ステップ: step_id
/// - 並列サブステップ: "parent_id/sub_id"
pub fn executable_items(wf: &Workflow, state: &WorkflowState) -> Vec<String> {
    let mut items = Vec::new();

    for step in &wf.steps {
        let step_state = state.steps.get(&step.id);
        let step_status = step_state.map(|s| &s.status).unwrap_or(&StepStatus::Pending);

        // 完了済み・失敗はスキップ
        if matches!(step_status, StepStatus::Completed | StepStatus::Failed) {
            continue;
        }

        // requires が全て完了しているか確認
        if !requires_met(wf, state, &step.requires) {
            continue;
        }

        if let Some(parallel) = &step.parallel {
            // 並列ブロック: pending のサブステップを全て追加
            for sub in parallel {
                let key = format!("{}/{}", step.id, sub.id);
                let sub_status = state.steps.get(&key)
                    .map(|s| &s.status)
                    .unwrap_or(&StepStatus::Pending);
                if matches!(sub_status, StepStatus::Pending | StepStatus::InProgress) {
                    items.push(key);
                }
            }
        } else {
            // 通常ステップ（手動ステップも含む）
            if matches!(step_status, StepStatus::Pending | StepStatus::InProgress) {
                items.push(step.id.clone());
            }
        }
    }

    items
}

fn requires_met(wf: &Workflow, state: &WorkflowState, requires: &[String]) -> bool {
    requires.iter().all(|req| {
        // requires は step_id を指す（並列サブステップは requires に使えない）
        state.steps.get(req)
            .map(|s| s.status == StepStatus::Completed)
            .unwrap_or(false)
            || wf.steps.iter().find(|s| &s.id == req).is_none() // 未知のステップはスルー
    })
}

pub fn is_workflow_complete(wf: &Workflow, state: &WorkflowState) -> bool {
    wf.steps.iter().all(|step| {
        state.steps.get(&step.id)
            .map(|s| s.status == StepStatus::Completed)
            .unwrap_or(false)
    })
}

/// step_id が並列サブステップか判定し、親 step_id を返す
pub fn parent_of(step_id: &str) -> Option<&str> {
    step_id.find('/').map(|i| &step_id[..i])
}
