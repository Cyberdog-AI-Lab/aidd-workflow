use serde::Deserialize;

/// Request body for POST /complete/:workflow_id/*task_id.
#[derive(Debug, Deserialize, Default)]
pub struct CompleteInput {
    pub summary: Option<String>,
}

/// Request body for POST /pause/:workflow_id/*task_id.
#[derive(Debug, Deserialize, Default)]
pub struct PauseInput {
    pub reason: Option<String>,
}

/// Request body for POST /report/:workflow_id/*task_id.
#[derive(Debug, Deserialize, Default)]
pub struct ReportInput {
    pub summary: Option<String>,
}

/// Request body for POST /run.
#[derive(Debug, Deserialize)]
pub struct RunInput {
    pub workflow: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_report_input() {
        let json = r#"{"summary":"ran tests, all passed"}"#;
        let input: ReportInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.summary.as_deref(), Some("ran tests, all passed"));
    }

    #[test]
    fn deserialize_report_input_empty_body() {
        let input: ReportInput = serde_json::from_str("{}").unwrap();
        assert!(input.summary.is_none());
    }
}
