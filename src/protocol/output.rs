use serde::Serialize;
use std::collections::HashMap;
use crate::engine::state::WorkflowState;
use crate::config::types::Workflow;

#[derive(Debug, Serialize)]
pub struct WorkflowOutput {
    pub session_id: String,
    pub workflow: String,
    pub status: FlowStatus,
    /// 次に実行すべきアクション群（並列実行可能なものはまとめて含まれる）
    pub actions: Vec<ActionItem>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowStatus {
    Started,
    InProgress,
    Completed,
    Blocked,
}

#[derive(Debug, Serialize)]
pub struct ActionItem {
    pub step_id: String,
    pub action_index: usize,
    /// step の name（表示用）
    pub step_name: String,
    #[serde(flatten)]
    pub action: ResolvedAction,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResolvedAction {
    Run {
        command: String,
        gate: bool,
    },
    Agent {
        prompt: String,
        background: bool,
    },
    Skill {
        skill: String,
        args: Vec<String>,
    },
    Workflow {
        workflow: String,
        inputs: HashMap<String, String>,
    },
    /// actions も parallel も持たない手動ステップ
    Manual {
        description: String,
        checklist_key: Option<String>,
    },
}

#[derive(Debug, Serialize)]
pub struct CompleteOutput {
    pub step_id: String,
    pub allowed: bool,
    pub reason: Option<String>,
    /// allowed = true のとき、次のアクション群
    pub next: Option<WorkflowOutput>,
}

#[derive(Debug, Serialize)]
pub struct StatusOutput {
    pub session_id: String,
    pub workflow: String,
    pub started_at: String,
    pub steps: Vec<StepStatusItem>,
}

#[derive(Debug, Serialize)]
pub struct StepStatusItem {
    pub id: String,
    pub name: String,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct WorkflowListItem {
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    pub step_count: usize,
}

#[derive(Debug, Serialize)]
pub struct ErrorOutput {
    pub error: String,
}

pub fn build_status(state: &WorkflowState, wf: &Workflow) -> StatusOutput {
    let steps = wf.steps.iter().map(|step| {
        let status = state.steps.get(&step.id)
            .map(|s| format!("{:?}", s.status).to_lowercase())
            .unwrap_or_else(|| "pending".to_string());
        StepStatusItem { id: step.id.clone(), name: step.name.clone(), status }
    }).collect();

    StatusOutput {
        session_id: state.session_id.clone(),
        workflow: state.workflow.clone(),
        started_at: state.started_at.to_rfc3339(),
        steps,
    }
}
