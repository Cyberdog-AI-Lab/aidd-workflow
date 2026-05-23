use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Root config.yml or any import file for workflow-orchestrator.
/// All top-level keys are optional in individual files; they are merged before validation.
#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Additional YAML files to merge, relative to .workflow/
    #[serde(default)]
    pub imports: Vec<String>,
    /// Named shell commands. Use `{{commands.<key>}}` in action prompts for interpolation.
    #[serde(default)]
    pub commands: HashMap<String, String>,
    #[serde(default)]
    pub workflows: HashMap<String, Workflow>,
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Workflow {
    pub name: String,
    pub description: Option<String>,
    /// Tasks to execute. At least one task is required (checked at runtime).
    #[schemars(schema_with = "non_empty_tasks")]
    pub tasks: Vec<Task>,
}

/// One task in a workflow.
/// Holds either `actions` or `agents`, never both.
/// A task with neither is a manual task: Claude works from `description`.
#[derive(Debug, Deserialize, Serialize, Clone, Default, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Task {
    /// Unique task identifier within the workflow. Pattern: `^[a-z][a-z0-9_-]*$`
    #[schemars(schema_with = "kebab_id")]
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// Actions to run automatically. Mutually exclusive with `agents` (checked at runtime).
    #[serde(default)]
    pub actions: Vec<Action>,
    /// Sub-agents to run concurrently. Mutually exclusive with `actions` (checked at runtime).
    pub agents: Option<Vec<SubAgentTask>>,
    /// IDs of tasks that must complete before this task starts (checked at runtime).
    #[serde(default)]
    pub requires: Vec<String>,
    /// File path patterns permitted for editing while InProgress (glob or /regex/).
    #[serde(default)]
    pub allow_files: Vec<String>,
    /// Explicit deny rules for files and shell commands.
    #[serde(default)]
    pub deny: Option<DenyRules>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DenyRules {
    /// File path patterns forbidden from editing (glob or /regex/).
    #[serde(default)]
    pub files: Vec<String>,
    /// Shell command patterns forbidden from running (substring match).
    #[serde(default)]
    pub commands: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubAgentTask {
    /// Unique sub-agent identifier within the agents block. Pattern: `^[a-z][a-z0-9_-]*$`
    #[schemars(schema_with = "kebab_id")]
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub actions: Vec<Action>,
    /// IDs of other sub-agents within the same agents block that must complete first
    /// (checked at runtime).
    #[serde(default)]
    pub requires: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    Agent {
        prompt: String,
        /// When true, may run concurrently with other actions in the same task.
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

// Custom schema functions for constraints that schemars cannot derive automatically.

fn non_empty_tasks(gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
    use schemars::schema::{ArrayValidation, SchemaObject, SingleOrVec};
    let item = gen.subschema_for::<Task>();
    SchemaObject {
        instance_type: Some(schemars::schema::InstanceType::Array.into()),
        array: Some(Box::new(ArrayValidation {
            items: Some(SingleOrVec::Single(Box::new(item))),
            min_items: Some(1),
            ..Default::default()
        })),
        ..Default::default()
    }
    .into()
}

fn kebab_id(_gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
    use schemars::schema::{SchemaObject, StringValidation};
    SchemaObject {
        instance_type: Some(schemars::schema::InstanceType::String.into()),
        string: Some(Box::new(StringValidation {
            pattern: Some("^[a-z][a-z0-9_-]*$".to_string()),
            ..Default::default()
        })),
        ..Default::default()
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_defaults_empty_requires_and_actions() {
        let yaml = r#"id: task1
name: Task 1"#;
        let task: Task = serde_yaml::from_str(yaml).unwrap();
        assert!(task.requires.is_empty());
        assert!(task.actions.is_empty());
        assert!(task.agents.is_none());
        assert!(task.allow_files.is_empty());
        assert!(task.deny.is_none());
    }

    #[test]
    fn agent_task_requires_defaults_to_empty() {
        let yaml = r#"id: sub1"#;
        let sub: SubAgentTask = serde_yaml::from_str(yaml).unwrap();
        assert!(sub.requires.is_empty());
    }

    #[test]
    fn agent_task_requires_parses_list() {
        let yaml = r#"id: sub2
requires: [sub1]"#;
        let sub: SubAgentTask = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(sub.requires, vec!["sub1"]);
    }

    #[test]
    fn task_parses_allow_files_and_deny() {
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
        let task: Task = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(task.allow_files.len(), 2);
        let deny = task.deny.unwrap();
        assert_eq!(deny.files, vec!["docs/specs/**"]);
        assert_eq!(deny.commands, vec!["git push"]);
    }

    #[test]
    fn config_defaults_empty_imports() {
        let yaml = r#"commands:
  test: make test
workflows: {}"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(config.imports.is_empty());
    }

    #[test]
    fn config_rejects_unknown_field() {
        let yaml = r#"unknown_key: value
workflows: {}"#;
        let result: Result<Config, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn task_rejects_unknown_field() {
        let yaml = r#"id: s1
name: S1
unknown_key: value"#;
        let result: Result<Task, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err());
    }

    /// Verifies that .workflow/workflow.schema.json matches the schema generated from Rust types.
    /// If this test fails, regenerate with: workflow-runner dump-schema > .workflow/workflow.schema.json
    #[test]
    fn schema_file_matches_generated() {
        let schema = schemars::schema_for!(Config);
        let generated: serde_json::Value = serde_json::to_value(&schema).unwrap();

        let schema_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".workflow/workflow.schema.json");
        let on_disk_str = std::fs::read_to_string(&schema_path).unwrap_or_else(|_| {
            panic!(
                "schema file not found; generate it with: workflow-runner dump-schema > .workflow/workflow.schema.json"
            )
        });
        let on_disk: serde_json::Value = serde_json::from_str(&on_disk_str).unwrap();

        assert_eq!(
            generated, on_disk,
            "schema is stale; regenerate with: workflow-runner dump-schema > .workflow/workflow.schema.json"
        );
    }
}
