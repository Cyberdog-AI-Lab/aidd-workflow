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
    /// Steps to execute. At least one step is required (checked at runtime).
    #[schemars(schema_with = "non_empty_steps")]
    pub steps: Vec<Step>,
}

/// One step in a workflow.
/// Holds either `actions` or `parallel`, never both.
/// A step with neither is a manual step: Claude works from `description`.
#[derive(Debug, Deserialize, Serialize, Clone, Default, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Step {
    /// Unique step identifier within the workflow. Pattern: `^[a-z][a-z0-9_-]*$`
    #[schemars(schema_with = "kebab_id")]
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// Actions to run automatically. Mutually exclusive with `parallel` (checked at runtime).
    #[serde(default)]
    pub actions: Vec<Action>,
    /// Sub-steps to run in parallel. Mutually exclusive with `actions` (checked at runtime).
    pub parallel: Option<Vec<SubStep>>,
    /// IDs of steps that must complete before this step starts (checked at runtime).
    #[serde(default)]
    pub requires: Vec<String>,
    /// File path patterns permitted for editing while InProgress (glob or /regex/).
    #[serde(default)]
    pub allow_files: Vec<String>,
    /// Explicit deny rules for files and shell commands.
    #[serde(default)]
    pub deny: Option<DenyRules>,
    /// Preconditions that must hold before this step can begin.
    #[serde(default)]
    pub guards: Vec<Guard>,
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
pub struct Guard {
    /// ID of a step that must have completed (checked at runtime).
    pub step: String,
    /// Files that must exist before this step can begin (glob or /regex/).
    #[serde(default)]
    pub required_files: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubStep {
    /// Unique sub-step identifier within the parallel block. Pattern: `^[a-z][a-z0-9_-]*$`
    #[schemars(schema_with = "kebab_id")]
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub actions: Vec<Action>,
    /// IDs of other sub-steps within the same parallel block that must complete first
    /// (checked at runtime).
    #[serde(default)]
    pub requires: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    Agent {
        prompt: String,
        /// When true, may run concurrently with other actions in the same step.
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

fn non_empty_steps(gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
    use schemars::schema::{ArrayValidation, SchemaObject, SingleOrVec};
    let item = gen.subschema_for::<Step>();
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
    fn step_defaults_empty_requires_and_actions() {
        let yaml = r#"id: step1
name: Step 1"#;
        let step: Step = serde_yaml::from_str(yaml).unwrap();
        assert!(step.requires.is_empty());
        assert!(step.actions.is_empty());
        assert!(step.parallel.is_none());
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

    #[test]
    fn config_rejects_unknown_field() {
        let yaml = r#"unknown_key: value
workflows: {}"#;
        let result: Result<Config, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn step_rejects_unknown_field() {
        let yaml = r#"id: s1
name: S1
unknown_key: value"#;
        let result: Result<Step, _> = serde_yaml::from_str(yaml);
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
