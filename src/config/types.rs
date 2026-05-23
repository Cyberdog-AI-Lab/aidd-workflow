use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Root config.yml or any import file for workflow-runner.
/// All top-level keys are optional in individual files; they are merged before validation.
#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Additional YAML files to merge, relative to .workflow/
    #[serde(default)]
    pub imports: Vec<String>,
    /// Named variables. Use `{{vars.<key>}}` in task prompts for interpolation.
    #[serde(default)]
    pub vars: HashMap<String, String>,
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
///
/// Task modes (mutually exclusive; checked at runtime):
/// - `prompt` and/or `skills`: automated task executed by an agent.
/// - `agents`: parallel custom-agent task; each element is a name under `.claude/agents/`.
/// - Neither: manual task — Claude works from `description` (required when manual).
#[derive(Debug, Deserialize, Serialize, Clone, Default, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Task {
    /// Unique task identifier within the workflow. Pattern: `^[a-z][a-z0-9_-]*$`
    #[schemars(schema_with = "kebab_id")]
    pub id: String,
    /// Concise task name shown in the task list. Required for manual tasks.
    pub task: Option<String>,
    /// Prompt sent to the agent. Supports `{{vars.<key>}}` interpolation.
    /// Mutually exclusive with `agents`.
    pub prompt: Option<String>,
    /// Names of skills to invoke for this task. Mutually exclusive with `agents`.
    #[serde(default)]
    pub skills: Vec<String>,
    /// Names of custom agents (defined in `.claude/agents/<name>.md`) to spawn in parallel.
    /// Mutually exclusive with `prompt` and `skills`.
    #[serde(default)]
    pub agents: Vec<String>,
    /// IDs of tasks that must complete before this task starts (checked at runtime).
    #[serde(default)]
    pub requires: Vec<String>,
    /// File path patterns the agent is expected to create or modify (glob or /regex/).
    /// Non-empty list restricts editing to matching paths only while InProgress.
    #[serde(default)]
    pub outputs: Vec<String>,
    /// Explicit deny rules for files and shell commands.
    #[serde(default)]
    pub deny: Option<DenyRules>,
    /// If true, pause the workflow after this task completes and wait for developer approval.
    #[serde(default)]
    pub approval: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DenyRules {
    /// File path patterns forbidden from editing (glob or /regex/).
    #[serde(default)]
    pub files: Vec<String>,
    /// Shell command patterns forbidden from running (substring or /regex/ match).
    #[serde(default)]
    pub commands: Vec<String>,
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
    fn task_defaults_are_empty() {
        let yaml = r#"id: task1"#;
        let task: Task = serde_yaml::from_str(yaml).unwrap();
        assert!(task.requires.is_empty());
        assert!(task.agents.is_empty());
        assert!(task.skills.is_empty());
        assert!(task.outputs.is_empty());
        assert!(task.deny.is_none());
        assert!(!task.approval);
        assert!(task.prompt.is_none());
        assert!(task.task.is_none());
    }

    #[test]
    fn task_parses_prompt_and_skills() {
        let yaml = r#"id: impl
task: Implement the feature
prompt: "Do the implementation"
skills:
  - security-review
approval: true"#;
        let task: Task = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(task.prompt.as_deref(), Some("Do the implementation"));
        assert_eq!(task.skills, vec!["security-review"]);
        assert!(task.approval);
        assert_eq!(task.task.as_deref(), Some("Implement the feature"));
    }

    #[test]
    fn task_parses_agents() {
        let yaml = r#"id: parallel
agents:
  - run-test
  - run-lint"#;
        let task: Task = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(task.agents, vec!["run-test", "run-lint"]);
    }

    #[test]
    fn task_parses_outputs_and_deny() {
        let yaml = r#"id: s
outputs:
  - "src/**"
  - "/.*\\.md$/"
deny:
  files:
    - "docs/specs/**"
  commands:
    - "git push""#;
        let task: Task = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(task.outputs.len(), 2);
        let deny = task.deny.unwrap();
        assert_eq!(deny.files, vec!["docs/specs/**"]);
        assert_eq!(deny.commands, vec!["git push"]);
    }

    #[test]
    fn config_defaults_empty_imports() {
        let yaml = r#"vars:
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
