use serde::Deserialize;

/// `workflow-runner report` の stdin JSON
#[derive(Debug, Deserialize)]
pub struct ReportInput {
    pub session_id: String,
    pub step_id: String,
    pub action_index: usize,
    pub action_type: String,
    pub exit_code: Option<i32>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
}
