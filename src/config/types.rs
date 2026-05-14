use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    pub commands: HashMap<String, String>,
    pub workflows: HashMap<String, Workflow>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Workflow {
    pub name: String,
    pub description: Option<String>,
    pub steps: Vec<Step>,
}

/// One step in a workflow.
/// Holds either `actions` or `parallel`, never both.
/// A step with neither is a manual step: Claude works from `description`.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Step {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub actions: Vec<Action>,
    pub parallel: Option<Vec<SubStep>>,
    pub checklist_key: Option<String>,
    #[serde(default)]
    pub requires: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SubStep {
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub actions: Vec<Action>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    Run {
        command: String,
        /// When true, a recorded execution of this action is required before `complete` is allowed.
        #[serde(default)]
        gate: bool,
    },
    Agent {
        prompt: String,
        /// When true, this action may run in parallel with other actions.
        #[serde(default)]
        background: bool,
    },
    Skill {
        skill: String,
        #[serde(default)]
        args: Vec<String>,
    },
    Workflow {
        workflow: String,
        #[serde(default)]
        inputs: HashMap<String, String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_run_defaults_gate_to_false() {
        let yaml = r#"type: run
command: make test"#;
        let action: Action = serde_yaml::from_str(yaml).unwrap();
        match action {
            Action::Run { gate, .. } => assert!(!gate),
            _ => panic!("expected Run"),
        }
    }

    #[test]
    fn action_run_gate_true() {
        let yaml = r#"type: run
command: make test
gate: true"#;
        let action: Action = serde_yaml::from_str(yaml).unwrap();
        match action {
            Action::Run { gate, .. } => assert!(gate),
            _ => panic!("expected Run"),
        }
    }

    #[test]
    fn step_defaults_empty_requires_and_actions() {
        let yaml = r#"id: step1
name: Step 1"#;
        let step: Step = serde_yaml::from_str(yaml).unwrap();
        assert!(step.requires.is_empty());
        assert!(step.actions.is_empty());
        assert!(step.parallel.is_none());
    }
}
