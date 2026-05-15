use super::types::{Action, Config};
use anyhow::{bail, Context, Result};
use std::path::Path;

pub fn load_config(cwd: &Path) -> Result<Config> {
    let path = cwd.join(".workflow/config.yml");
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!(".workflow/config.yml not found: {}", path.display()))?;
    let config: Config = serde_yaml::from_str(&content).context("failed to parse config.yml")?;
    validate(&config)?;
    Ok(config)
}

pub fn validate(config: &Config) -> Result<()> {
    if !config.commands.contains_key("test") {
        bail!("commands.test is not defined in config.yml");
    }
    for (slug, wf) in &config.workflows {
        if wf.steps.is_empty() {
            bail!("workflow '{}' has no steps", slug);
        }
        let ids: std::collections::HashSet<&str> = wf.steps.iter().map(|s| s.id.as_str()).collect();

        for step in &wf.steps {
            if !step.actions.is_empty() && step.parallel.is_some() {
                bail!(
                    "step '{}' in workflow '{}' cannot have both actions and parallel",
                    step.id,
                    slug
                );
            }
            for req in &step.requires {
                if !ids.contains(req.as_str()) {
                    bail!(
                        "step '{}' in workflow '{}' requires unknown step '{}'",
                        step.id,
                        slug,
                        req
                    );
                }
            }
            // Validate that run action commands are non-empty
            for action in &step.actions {
                if let Action::Run { command, .. } = action {
                    if command.is_empty() {
                        bail!(
                            "step '{}' in workflow '{}' has a run action with an empty command",
                            step.id,
                            slug
                        );
                    }
                }
            }
        }
    }
    Ok(())
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
}
