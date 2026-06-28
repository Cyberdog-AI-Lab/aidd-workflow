use super::types::Config;
use anyhow::{Context, Result};
use regex::Regex;
use std::collections::HashSet;
use std::path::Path;
use std::sync::LazyLock;

static TASK_ID_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z][a-z0-9_-]*$").unwrap());

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

/// Loads and merges config from `.workflow/config.yml` and any `imports`,
/// but does NOT run validation. Use this when you need the merged config
/// before calling `validate()` separately (e.g. to collect detailed errors).
pub fn load_and_merge_config(cwd: &Path) -> Result<Config> {
    let root = cwd.join(".workflow/config.yml");
    load_config_recursive(&root, cwd, &mut HashSet::new())
}

/// Loads, merges, and validates config from `.workflow/config.yml` and any `imports`.
/// Returns an error if the file is missing, unparseable, or fails validation.
pub fn load_config(cwd: &Path) -> Result<Config> {
    let config = load_and_merge_config(cwd)?;
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

    // `visited` tracks the current DFS stack, not globally-visited nodes.
    // This allows diamond imports (A→B→D, A→C→D) while still detecting true cycles.
    if !visited.insert(canonical.clone()) {
        anyhow::bail!("circular import detected: {}", path.display());
    }

    let content = std::fs::read_to_string(&canonical)
        .with_context(|| format!("cannot read config: {}", canonical.display()))?;

    let mut config: Config =
        serde_yaml::from_str(&content).context("failed to parse config YAML")?;

    // Resolve and merge imports before returning; validate only at the top level.
    let imports = std::mem::take(&mut config.imports);

    // Compute the canonical .workflow/ directory once for path-traversal checks.
    let workflow_dir = base
        .join(".workflow")
        .canonicalize()
        .with_context(|| format!("cannot resolve .workflow/ under {}", base.display()))?;

    for import_path in &imports {
        let abs = base.join(".workflow").join(import_path);

        // Reject imports that escape the .workflow/ directory (path traversal).
        let import_canonical = abs
            .canonicalize()
            .with_context(|| format!("import not found: {}", import_path))?;
        if !import_canonical.starts_with(&workflow_dir) {
            anyhow::bail!("import '{}' escapes .workflow/ directory", import_path);
        }

        let child = load_config_recursive(&abs, base, visited)?;
        merge_into(&mut config, child);
    }

    // Pop this node from the DFS stack so sibling branches can import the same file.
    visited.remove(&canonical);
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

        // Detect duplicate task IDs (HashSet deduplicates; size mismatch reveals duplicates).
        let ids: HashSet<&str> = wf.tasks.iter().map(|s| s.id.as_str()).collect();
        if ids.len() != wf.tasks.len() {
            let mut seen: HashSet<&str> = HashSet::new();
            for task in &wf.tasks {
                if !seen.insert(task.id.as_str()) {
                    errors.push(format!(
                        "workflow '{}': duplicate task id '{}'",
                        slug, task.id
                    ));
                }
            }
        }

        for task in &wf.tasks {
            // Task ID must match the required pattern (must not contain '/' or upper-case).
            if !TASK_ID_RE.is_match(&task.id) {
                errors.push(format!(
                    "task '{}' in workflow '{}': id must match ^[a-z][a-z0-9_-]*$",
                    task.id, slug
                ));
            }

            // prompt/skills and agents are mutually exclusive.
            if (task.prompt.is_some() || !task.skills.is_empty()) && !task.agents.is_empty() {
                errors.push(format!(
                    "task '{}' in workflow '{}': 'prompt'/'skills' and 'agents' are mutually exclusive",
                    task.id, slug
                ));
            }

            // Manual tasks (no prompt, skills, or agents) require a task name.
            let is_manual =
                task.prompt.is_none() && task.skills.is_empty() && task.agents.is_empty();
            if is_manual && task.task.is_none() {
                errors.push(format!(
                    "task '{}' in workflow '{}': manual task requires 'task'",
                    task.id, slug
                ));
            }

            // All requires must reference known task IDs.
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
            task: Some("Do task 1".to_string()),
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
    fn validate_rejects_prompt_and_agents_together() {
        let mut config = minimal_config();
        let wf = config.workflows.get_mut("wf").unwrap();
        wf.tasks[0].prompt = Some("do it".to_string());
        wf.tasks[0].agents = vec!["some-agent".to_string()];
        assert!(validate(&config).is_err());
    }

    #[test]
    fn validate_rejects_skills_and_agents_together() {
        let mut config = minimal_config();
        let wf = config.workflows.get_mut("wf").unwrap();
        wf.tasks[0].skills = vec!["security-review".to_string()];
        wf.tasks[0].agents = vec!["some-agent".to_string()];
        assert!(validate(&config).is_err());
    }

    #[test]
    fn validate_rejects_manual_task_without_task_name() {
        let mut config = minimal_config();
        let wf = config.workflows.get_mut("wf").unwrap();
        wf.tasks[0].task = None; // remove task name from manual task
        let err = validate(&config).unwrap_err();
        assert!(err.to_string().contains("manual task requires 'task'"));
    }

    #[test]
    fn validate_accepts_prompt_task_without_task_name() {
        let mut config = minimal_config();
        let wf = config.workflows.get_mut("wf").unwrap();
        wf.tasks[0].task = None;
        wf.tasks[0].prompt = Some("Do something".to_string());
        assert!(validate(&config).is_ok());
    }

    #[test]
    fn validate_accepts_agents_task_without_task_name() {
        let mut config = minimal_config();
        let wf = config.workflows.get_mut("wf").unwrap();
        wf.tasks[0].task = None;
        wf.tasks[0].agents = vec!["run-test".to_string()];
        assert!(validate(&config).is_ok());
    }

    #[test]
    fn validate_collects_multiple_errors() {
        let mut config = minimal_config();
        let wf = config.workflows.get_mut("wf").unwrap();
        wf.tasks[0].requires.push("missing".to_string());
        wf.tasks[0].prompt = Some("do it".to_string());
        wf.tasks[0].agents = vec!["some-agent".to_string()];
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
        task: Extra task 1
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
        task: Main task 1
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
        task: Step 1
"#,
        )
        .unwrap();

        let result = load_config(dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("circular"));
    }

    /// Diamond import: A→[B,C], B→shared, C→shared must succeed (not be treated as circular).
    #[test]
    fn load_config_resolves_diamond_imports() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let wf_dir = dir.path().join(".workflow");
        std::fs::create_dir_all(&wf_dir).unwrap();

        std::fs::write(
            wf_dir.join("shared.yml"),
            r#"vars:
  shared_var: shared_value
workflows: {}"#,
        )
        .unwrap();
        std::fs::write(
            wf_dir.join("a.yml"),
            r#"imports:
  - shared.yml
workflows: {}"#,
        )
        .unwrap();
        std::fs::write(
            wf_dir.join("b.yml"),
            r#"imports:
  - shared.yml
workflows: {}"#,
        )
        .unwrap();
        std::fs::write(
            wf_dir.join("config.yml"),
            r#"imports:
  - a.yml
  - b.yml
workflows:
  wf:
    name: WF
    tasks:
      - id: s1
        task: Step 1
"#,
        )
        .unwrap();

        let config = load_config(dir.path()).unwrap();
        assert!(config.vars.contains_key("shared_var"));
        assert!(config.workflows.contains_key("wf"));
    }

    #[test]
    fn validate_rejects_duplicate_task_ids() {
        let mut config = minimal_config();
        let wf = config.workflows.get_mut("wf").unwrap();
        wf.tasks.push(Task {
            id: "task1".to_string(), // duplicate of the existing "task1"
            task: Some("Duplicate task".to_string()),
            ..Task::default()
        });
        let err = validate(&config).unwrap_err();
        assert!(
            err.to_string().contains("duplicate task id"),
            "expected duplicate task id error, got: {}",
            err
        );
    }

    #[test]
    fn validate_rejects_invalid_task_id_with_slash() {
        let mut config = minimal_config();
        let wf = config.workflows.get_mut("wf").unwrap();
        wf.tasks[0].id = "my/task".to_string();
        let err = validate(&config).unwrap_err();
        assert!(
            err.to_string().contains("id must match"),
            "expected pattern error, got: {}",
            err
        );
    }

    #[test]
    fn validate_rejects_invalid_task_id_uppercase() {
        let mut config = minimal_config();
        let wf = config.workflows.get_mut("wf").unwrap();
        wf.tasks[0].id = "MyTask".to_string();
        let err = validate(&config).unwrap_err();
        assert!(err.to_string().contains("id must match"));
    }

    #[test]
    fn load_config_rejects_path_traversal_import() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let wf_dir = dir.path().join(".workflow");
        std::fs::create_dir_all(&wf_dir).unwrap();

        // Create a file outside .workflow/ that could be targeted.
        std::fs::write(dir.path().join("outside.yml"), "workflows: {}").unwrap();

        std::fs::write(
            wf_dir.join("config.yml"),
            r#"imports:
  - ../outside.yml
workflows:
  wf:
    name: WF
    tasks:
      - id: s1
        task: Step 1
"#,
        )
        .unwrap();

        let result = load_config(dir.path());
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("escapes"),
            "expected path traversal error"
        );
    }
}
