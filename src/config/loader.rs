use super::types::{Action, Config};
use anyhow::{Context, Result};
use std::path::Path;

/// Accumulates all validation errors found in a config, rather than stopping at the first.
#[derive(Debug)]
pub struct ValidationError {
    pub errors: Vec<String>,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, e) in self.errors.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "[{}] {}", i + 1, e)?;
        }
        Ok(())
    }
}

impl std::error::Error for ValidationError {}

pub fn load_config(cwd: &Path) -> Result<Config> {
    let path = cwd.join(".workflow/config.yml");
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!(".workflow/config.yml not found: {}", path.display()))?;
    let config: Config = serde_yaml::from_str(&content).context("failed to parse config.yml")?;
    validate(&config).map_err(|e| anyhow::anyhow!("{}", e))?;
    Ok(config)
}

/// Validates the parsed config and collects all errors before returning.
/// Returns `Err(ValidationError)` with every issue found, so callers can display all
/// problems at once rather than requiring repeated fix-and-retry cycles.
pub fn validate(config: &Config) -> Result<(), ValidationError> {
    let mut errors = Vec::new();

    if !config.commands.contains_key("test") {
        errors.push("commands.test is not defined in config.yml".to_string());
    }
    for (slug, wf) in &config.workflows {
        if wf.steps.is_empty() {
            errors.push(format!("workflow '{}' has no steps", slug));
        }
        let ids: std::collections::HashSet<&str> = wf.steps.iter().map(|s| s.id.as_str()).collect();

        for step in &wf.steps {
            if !step.actions.is_empty() && step.parallel.is_some() {
                errors.push(format!(
                    "step '{}' in workflow '{}': cannot have both actions and parallel",
                    step.id, slug
                ));
            }
            for req in &step.requires {
                if !ids.contains(req.as_str()) {
                    errors.push(format!(
                        "step '{}' in workflow '{}': unknown requires '{}'",
                        step.id, slug, req
                    ));
                }
            }
            for action in &step.actions {
                if let Action::Run { command, .. } = action {
                    if command.is_empty() {
                        errors.push(format!(
                            "step '{}' in workflow '{}': run action has empty command",
                            step.id, slug
                        ));
                    }
                }
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(ValidationError { errors })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{Config, Step, Workflow};
    use std::collections::HashMap;

    fn minimal_config() -> Config {
        let mut commands = HashMap::new();
        commands.insert("test".to_string(), "make test".to_string());

        let step = Step {
            id: "step1".to_string(),
            name: "Step 1".to_string(),
            description: None,
            actions: vec![],
            parallel: None,
            checklist_key: None,
            requires: vec![],
        };
        let mut workflows = HashMap::new();
        workflows.insert(
            "wf".to_string(),
            Workflow {
                name: "WF".to_string(),
                description: None,
                steps: vec![step],
            },
        );
        Config {
            commands,
            workflows,
        }
    }

    #[test]
    fn validate_accepts_minimal_config() {
        assert!(validate(&minimal_config()).is_ok());
    }

    #[test]
    fn validate_rejects_missing_test_command() {
        let mut config = minimal_config();
        config.commands.remove("test");
        assert!(validate(&config).is_err());
    }

    #[test]
    fn validate_rejects_unknown_requires() {
        let mut config = minimal_config();
        let wf = config.workflows.get_mut("wf").unwrap();
        wf.steps[0].requires.push("nonexistent".to_string());
        let err = validate(&config).unwrap_err();
        assert!(err.to_string().contains("nonexistent"));
    }

    #[test]
    fn validate_rejects_both_actions_and_parallel() {
        use crate::config::types::{Action, SubStep};
        let mut config = minimal_config();
        let wf = config.workflows.get_mut("wf").unwrap();
        wf.steps[0].actions = vec![Action::Run {
            command: "make test".to_string(),
            gate: false,
        }];
        wf.steps[0].parallel = Some(vec![SubStep {
            id: "sub1".to_string(),
            name: None,
            description: None,
            actions: vec![],
            requires: vec![],
        }]);
        assert!(validate(&config).is_err());
    }

    #[test]
    fn validate_collects_multiple_errors() {
        use crate::config::types::{Action, SubStep};
        let mut config = minimal_config();
        config.commands.remove("test");
        let wf = config.workflows.get_mut("wf").unwrap();
        wf.steps[0].requires.push("missing".to_string());
        wf.steps[0].actions = vec![Action::Run {
            command: "make test".to_string(),
            gate: false,
        }];
        wf.steps[0].parallel = Some(vec![SubStep {
            id: "sub1".to_string(),
            name: None,
            description: None,
            actions: vec![],
            requires: vec![],
        }]);
        let err = validate(&config).unwrap_err();
        assert!(
            err.errors.len() >= 3,
            "expected at least 3 errors, got {}: {:?}",
            err.errors.len(),
            err.errors
        );
    }

    #[test]
    fn validation_error_display_contains_all_messages() {
        let ve = ValidationError {
            errors: vec!["first problem".to_string(), "second problem".to_string()],
        };
        let s = ve.to_string();
        assert!(s.contains("first problem"));
        assert!(s.contains("second problem"));
        assert!(s.contains("[1]"));
        assert!(s.contains("[2]"));
    }
}
