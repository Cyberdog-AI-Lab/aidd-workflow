use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    #[serde(default)]
    pub imports: Vec<String>,
    #[serde(default)]
    pub commands: HashMap<String, String>,
    #[serde(default)]
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
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct Step {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub actions: Vec<Action>,
    pub parallel: Option<Vec<SubStep>>,
    #[serde(default)]
    pub requires: Vec<String>,
    /// Commands run automatically when the step becomes InProgress.
    #[serde(default)]
    pub pre_commands: Vec<String>,
    /// Commands run as a gate before Complete is allowed.
    #[serde(default)]
    pub post_commands: Vec<String>,
    /// File path patterns allowed for edit during this step (glob or /regex/).
    #[serde(default)]
    pub allow_files: Vec<String>,
    /// Explicit deny rules for files and shell commands.
    #[serde(default)]
    pub deny: Option<DenyRules>,
    /// Preconditions that must hold before this step can begin.
    #[serde(default)]
    pub guards: Vec<Guard>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct DenyRules {
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub commands: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Guard {
    /// The step that must have completed.
    pub step: String,
    /// Files that must exist (glob or /regex/ patterns).
    #[serde(default)]
    pub required_files: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SubStep {
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub actions: Vec<Action>,
    /// Dependencies on other sub-steps within the same parallel block.
    #[serde(default)]
    pub requires: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
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
    fn step_defaults_empty_requires_and_actions() {
        let yaml = r#"id: step1
name: Step 1"#;
        let step: Step = serde_yaml::from_str(yaml).unwrap();
        assert!(step.requires.is_empty());
        assert!(step.actions.is_empty());
        assert!(step.parallel.is_none());
        assert!(step.pre_commands.is_empty());
        assert!(step.post_commands.is_empty());
        assert!(step.allow_files.is_empty());
        assert!(step.deny.is_none());
        assert!(step.guards.is_empty());
    }

    #[test]
    fn sub_step_requires_defaults_to_empty() {
        let yaml = r#"id: sub1"#;
        let sub: SubStep = serde_yaml::from_str(yaml).unwrap();
        assert!(sub.requires.is_empty());
    }

    #[test]
    fn sub_step_requires_parses_list() {
        let yaml = r#"id: sub2
requires: [sub1]"#;
        let sub: SubStep = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(sub.requires, vec!["sub1"]);
    }

    #[test]
    fn step_parses_pre_post_commands() {
        let yaml = r#"id: impl
name: Implement
pre_commands:
  - cargo check
post_commands:
  - cargo test"#;
        let step: Step = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(step.pre_commands, vec!["cargo check"]);
        assert_eq!(step.post_commands, vec!["cargo test"]);
    }

    #[test]
    fn step_parses_allow_files_and_deny() {
        let yaml = r#"id: s
name: S
allow_files:
  - "src/**"
  - "/.*\\.md$/"
deny:
  files:
    - "docs/specs/**"
  commands:
    - "git push""#;
        let step: Step = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(step.allow_files.len(), 2);
        let deny = step.deny.unwrap();
        assert_eq!(deny.files, vec!["docs/specs/**"]);
        assert_eq!(deny.commands, vec!["git push"]);
    }

    #[test]
    fn step_parses_guards() {
        let yaml = r#"id: impl
name: Implement
guards:
  - step: design
    required_files:
      - "docs/**/*.md""#;
        let step: Step = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(step.guards.len(), 1);
        assert_eq!(step.guards[0].step, "design");
        assert_eq!(step.guards[0].required_files, vec!["docs/**/*.md"]);
    }

    #[test]
    fn config_defaults_empty_imports() {
        let yaml = r#"commands:
  test: make test
workflows: {}"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(config.imports.is_empty());
    }
}
