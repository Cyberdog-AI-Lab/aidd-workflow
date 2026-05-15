use serde::Deserialize;

/// Stdin JSON for `workflow-runner report`.
#[derive(Debug, Deserialize)]
pub struct ReportInput {
    // Accepted but not validated; reserved for future session verification.
    #[allow(dead_code)]
    pub session_id: String,
    pub step_id: String,
    pub action_index: usize,
    pub action_type: String,
    pub exit_code: Option<i32>,
    pub stdout: Option<String>,
    // Accepted for protocol symmetry; not stored in state.
    #[allow(dead_code)]
    pub stderr: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_report_input() {
        let json = r#"{
            "session_id": "abc",
            "step_id": "test",
            "action_index": 0,
            "action_type": "run",
            "exit_code": 0,
            "stdout": "ok",
            "stderr": null
        }"#;
        let input: ReportInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.step_id, "test");
        assert_eq!(input.exit_code, Some(0));
    }

    #[test]
    fn deserialize_report_input_optional_fields_nullable() {
        let json = r#"{
            "session_id": "x",
            "step_id": "s",
            "action_index": 1,
            "action_type": "agent"
        }"#;
        let input: ReportInput = serde_json::from_str(json).unwrap();
        assert!(input.exit_code.is_none());
        assert!(input.stdout.is_none());
    }
}
