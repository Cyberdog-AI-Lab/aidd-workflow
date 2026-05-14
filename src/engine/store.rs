use std::path::Path;
use anyhow::Result;
use super::state::WorkflowState;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{Workflow, Step};
    use crate::engine::state::WorkflowState;
    use tempfile::TempDir;

    fn minimal_workflow() -> Workflow {
        Workflow {
            name: "test".to_string(),
            description: None,
            steps: vec![
                Step {
                    id: "step1".to_string(),
                    name: "Step 1".to_string(),
                    description: None,
                    actions: vec![],
                    parallel: None,
                    checklist_key: None,
                    requires: vec![],
                },
            ],
        }
    }

    #[test]
    fn roundtrip_state() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path();
        std::fs::create_dir_all(cwd.join(".workflow")).unwrap();

        let wf = minimal_workflow();
        let state = WorkflowState::new("test", &wf);
        let session_id = state.session_id.clone();

        save_state(cwd, &state).unwrap();
        let loaded = load_state(cwd).unwrap().unwrap();
        assert_eq!(loaded.session_id, session_id);
        assert_eq!(loaded.workflow, "test");
    }

    #[test]
    fn load_state_returns_none_when_missing() {
        let dir = TempDir::new().unwrap();
        let result = load_state(dir.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn clear_state_removes_file() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path();
        std::fs::create_dir_all(cwd.join(".workflow")).unwrap();

        let wf = minimal_workflow();
        let state = WorkflowState::new("test", &wf);
        save_state(cwd, &state).unwrap();

        assert!(cwd.join(".workflow/state.json").exists());
        clear_state(cwd).unwrap();
        assert!(!cwd.join(".workflow/state.json").exists());
    }
}
