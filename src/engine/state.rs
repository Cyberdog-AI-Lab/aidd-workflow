use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use anyhow::Result;
use chrono::{DateTime, Utc};
use uuid::Uuid;
use crate::config::types::Workflow;

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

impl Default for StepStatus {
    fn default() -> Self {
        StepStatus::Pending
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct StepState {
    #[serde(default)]
    pub status: StepStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    /// gate: true の run アクションが実行・報告済みかどうか
    #[serde(default)]
    pub gate_recorded: bool,
    #[serde(default)]
    pub action_reports: Vec<ActionReport>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ActionReport {
    pub action_index: usize,
    pub action_type: String,
    pub exit_code: Option<i32>,
    pub stdout: Option<String>,
    pub recorded_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct WorkflowState {
    pub session_id: String,
    pub workflow: String,
    pub started_at: DateTime<Utc>,
    /// キー: step_id（並列サブステップは "parent_id/sub_id" 形式）
    pub steps: HashMap<String, StepState>,
}

impl WorkflowState {
    pub fn new(workflow_name: &str, wf: &Workflow) -> Self {
        let mut steps = HashMap::new();
        for step in &wf.steps {
            steps.insert(step.id.clone(), StepState::default());
            if let Some(parallel) = &step.parallel {
                for sub in parallel {
                    steps.insert(format!("{}/{}", step.id, sub.id), StepState::default());
                }
            }
        }
        WorkflowState {
            session_id: Uuid::new_v4().to_string(),
            workflow: workflow_name.to_string(),
            started_at: Utc::now(),
            steps,
        }
    }

    /// 並列親ステップのステータスをサブステップから導出して同期する
    pub fn sync_parallel_parent(&mut self, parent_id: &str, wf: &Workflow) {
        let parent_step = match wf.steps.iter().find(|s| s.id == parent_id) {
            Some(s) => s,
            None => return,
        };
        let parallel = match &parent_step.parallel {
            Some(p) => p,
            None => return,
        };

        let all_completed = parallel.iter().all(|sub| {
            let key = format!("{}/{}", parent_id, sub.id);
            self.steps.get(&key).map(|s| s.status == StepStatus::Completed).unwrap_or(false)
        });
        let any_started = parallel.iter().any(|sub| {
            let key = format!("{}/{}", parent_id, sub.id);
            self.steps.get(&key)
                .map(|s| s.status != StepStatus::Pending)
                .unwrap_or(false)
        });

        let parent = self.steps.entry(parent_id.to_string()).or_default();
        if all_completed {
            parent.status = StepStatus::Completed;
            if parent.completed_at.is_none() {
                parent.completed_at = Some(Utc::now());
            }
        } else if any_started {
            if parent.status == StepStatus::Pending {
                parent.status = StepStatus::InProgress;
                parent.started_at = Some(Utc::now());
            }
        }
    }
}

pub fn load_state(cwd: &Path) -> Result<Option<WorkflowState>> {
    let path = cwd.join(".workflow/state.json");
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)?;
    Ok(Some(serde_json::from_str(&content)?))
}

pub fn save_state(cwd: &Path, state: &WorkflowState) -> Result<()> {
    let dir = cwd.join(".workflow");
    std::fs::create_dir_all(&dir)?;
    let content = serde_json::to_string_pretty(state)?;
    std::fs::write(dir.join("state.json"), content)?;
    Ok(())
}

pub fn clear_state(cwd: &Path) -> Result<()> {
    let path = cwd.join(".workflow/state.json");
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}
