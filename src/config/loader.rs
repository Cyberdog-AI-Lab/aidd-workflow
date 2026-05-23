use super::types::Config;
use anyhow::{Context, Result};
use std::collections::HashSet;
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

/// Loads and merges config from `.workflow/config.yml` and any `imports`.
/// Validation (including `vars.test` check) runs only on the fully merged config.
pub fn load_config(cwd: &Path) -> Result<Config> {
    let root = cwd.join(".workflow/config.yml");
    let config = load_config_recursive(&root, cwd, &mut HashSet::new())?;
    validate(&config).map_err(|e| anyhow::anyhow!("{}", e))?;
    Ok(config)
}

fn load_config_recursive(
    path: &Path,
    base: &Path,
    visited: &mut HashSet<std::path::PathBuf>,
) -> Result<Config> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("config file not found: {}", path.display()))?;

    if !visited.insert(canonical.clone()) {
        anyhow::bail!("circular import detected: {}", path.display());
    }

    let content = std::fs::read_to_string(&canonical)
        .with_context(|| format!("cannot read config: {}", canonical.display()))?;

    let mut config: Config =
        serde_yaml::from_str(&content).context("failed to parse config YAML")?;

    // Resolve and merge imports before returning; validate only at the top level.
    let imports = std::mem::take(&mut config.imports);
    for import_path in &imports {
        let abs = base.join(".workflow").join(import_path);
        let child = load_config_recursive(&abs, base, visited)?;
        merge_into(&mut config, child);
    }

    Ok(config)
}

fn merge_into(base: &mut Config, child: Config) {
    for (k, v) in child.vars {
        base.vars.entry(k).or_insert(v);
    }
    for (k, v) in child.workflows {
        base.workflows.entry(k).or_insert(v);
    }
}

/// Validates the parsed config and collects all errors before returning.
pub fn validate(config: &Config) -> Result<(), ValidationError> {
    let mut errors = Vec::new();

    for (slug, wf) in &config.workflows {
        if wf.tasks.is_empty() {
            errors.push(format!("workflow '{}' has no tasks", slug));
        }
        let ids: HashSet<&str> = wf.tasks.iter().map(|s| s.id.as_str()).collect();

        for task in &wf.tasks {
            if !task.actions.is_empty() && task.agents.is_some() {
                errors.push(format!(
                    "task '{}' in workflow '{}': cannot have both actions and agents",
                    task.id, slug
                ));
            }
            for req in &task.requires {
                if !ids.contains(req.as_str()) {
                    errors.push(format!(
                        "task '{}' in workflow '{}': unknown requires '{}'",
                        task.id, slug, req
                    ));
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

/// Checks whether a single path string matches a pattern (glob or /regex/).
pub fn matches_pattern(pattern: &str, path: &str) -> bool {
    if let Some(inner) = pattern.strip_prefix('/').and_then(|s| s.strip_suffix('/')) {
        regex::Regex::new(inner)
            .map(|re| re.is_match(path))
            .unwrap_or(false)
    } else {
        glob::Pattern::new(pattern)
            .map(|p| p.matches(path))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{Task, Workflow};
    use std::collections::HashMap;

    fn minimal_config() -> Config {
        let task = Task {
            id: "task1".to_string(),
            name: "Task 1".to_string(),
            ..Task::default()
        };
        let mut workflows = HashMap::new();
        workflows.insert(
            "wf".to_string(),
            Workflow {
                name: "WF".to_string(),
                description: None,
                tasks: vec![task],
            },
        );
        Config {
            imports: vec![],
            vars: HashMap::new(),
            workflows,
        }
    }

    #[test]
    fn validate_accepts_minimal_config() {
        assert!(validate(&minimal_config()).is_ok());
    }

    #[test]
    fn validate_accepts_empty_vars() {
        let mut config = minimal_config();
        config.vars.clear();
        assert!(validate(&config).is_ok());
    }

    #[test]
    fn validate_rejects_unknown_requires() {
        let mut config = minimal_config();
        let wf = config.workflows.get_mut("wf").unwrap();
        wf.tasks[0].requires.push("nonexistent".to_string());
        let err = validate(&config).unwrap_err();
        assert!(err.to_string().contains("nonexistent"));
    }

    #[test]
    fn validate_rejects_both_actions_and_agents() {
        use crate::config::types::{Action, SubAgentTask};
        let mut config = minimal_config();
        let wf = config.workflows.get_mut("wf").unwrap();
        wf.tasks[0].actions = vec![Action::Agent {
            prompt: "do it".to_string(),
            background: false,
        }];
        wf.tasks[0].agents = Some(vec![SubAgentTask {
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
        use crate::config::types::{Action, SubAgentTask};
        let mut config = minimal_config();
        let wf = config.workflows.get_mut("wf").unwrap();
        wf.tasks[0].requires.push("missing".to_string());
        wf.tasks[0].actions = vec![Action::Agent {
            prompt: "do it".to_string(),
            background: false,
        }];
        wf.tasks[0].agents = Some(vec![SubAgentTask {
            id: "sub1".to_string(),
            name: None,
            description: None,
            actions: vec![],
            requires: vec![],
        }]);
        let err = validate(&config).unwrap_err();
        assert!(
            err.errors.len() >= 2,
            "expected at least 2 errors, got {}: {:?}",
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

    #[test]
    fn matches_pattern_glob() {
        assert!(matches_pattern("src/**", "src/main.rs"));
        assert!(!matches_pattern("src/**", "tests/foo.rs"));
    }

    #[test]
    fn matches_pattern_regex() {
        assert!(matches_pattern("/.*\\.md$/", "README.md"));
        assert!(!matches_pattern("/.*\\.md$/", "main.rs"));
    }

    #[test]
    fn load_config_rejects_action_run_via_yaml() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let workflow_dir = dir.path().join(".workflow");
        std::fs::create_dir_all(&workflow_dir).unwrap();

        let yaml = r#"
workflows:
  wf:
    name: WF
    tasks:
      - id: s
        name: S
        actions:
          - type: run
            command: make test
"#;
        std::fs::write(workflow_dir.join("config.yml"), yaml).unwrap();

        // type: run is no longer a valid Action variant; serde_yaml rejects it at parse time.
        let result = load_config(dir.path());
        assert!(
            result.is_err(),
            "expected parse error for deprecated Action::Run"
        );
    }

    #[test]
    fn load_config_resolves_imports() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let wf_dir = dir.path().join(".workflow");
        std::fs::create_dir_all(wf_dir.join("workflows")).unwrap();

        std::fs::write(
            wf_dir.join("workflows/extra.yml"),
            r#"vars:
  lint: make lint
workflows:
  extra:
    name: Extra
    tasks:
      - id: e1
        name: E1
"#,
        )
        .unwrap();

        std::fs::write(
            wf_dir.join("config.yml"),
            r#"imports:
  - workflows/extra.yml
workflows:
  main:
    name: Main
    tasks:
      - id: m1
        name: M1
"#,
        )
        .unwrap();

        let config = load_config(dir.path()).unwrap();
        assert!(config.workflows.contains_key("extra"));
        assert!(config.workflows.contains_key("main"));
        assert!(config.vars.contains_key("lint"));
    }

    #[test]
    fn load_config_detects_circular_imports() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let wf_dir = dir.path().join(".workflow");
        std::fs::create_dir_all(&wf_dir).unwrap();

        // a.yml imports b.yml, b.yml imports a.yml
        std::fs::write(
            wf_dir.join("a.yml"),
            r#"imports:
  - b.yml
workflows: {}"#,
        )
        .unwrap();
        std::fs::write(
            wf_dir.join("b.yml"),
            r#"imports:
  - a.yml
workflows: {}"#,
        )
        .unwrap();
        std::fs::write(
            wf_dir.join("config.yml"),
            r#"imports:
  - a.yml
workflows:
  wf:
    name: WF
    tasks:
      - id: s1
        name: S1
"#,
        )
        .unwrap();

        let result = load_config(dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("circular"));
    }
}
