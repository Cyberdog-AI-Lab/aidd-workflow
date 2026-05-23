use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::Path;

const WORKFLOW_RUNNER_CMD_PREFIX: &str = "workflow-runner hook";

/// Writes `.claude/settings.json` from scratch with workflow-runner hook entries.
/// Includes `.claude/hooks/post-edit-rust-checks.sh` if the file exists.
pub fn write_settings_json(cwd: &Path) -> Result<()> {
    let settings_dir = cwd.join(".claude");
    std::fs::create_dir_all(&settings_dir).context("failed to create .claude directory")?;

    let settings = build_settings(cwd);
    let json = serde_json::to_string_pretty(&settings)?;

    let path = settings_dir.join("settings.json");
    std::fs::write(&path, json + "\n")
        .with_context(|| format!("failed to write {}", path.display()))?;

    Ok(())
}

/// Reads existing `.claude/settings.json` and merges workflow-runner hook entries,
/// preserving all non-workflow-runner hooks.
pub fn merge_settings_json(cwd: &Path) -> Result<()> {
    let path = cwd.join(".claude/settings.json");

    let existing: Value = if path.exists() {
        let content =
            std::fs::read_to_string(&path).context("failed to read .claude/settings.json")?;
        serde_json::from_str(&content).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let target = build_settings(cwd);
    let merged = merge_hook_settings(existing, target);

    let json = serde_json::to_string_pretty(&merged)?;
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, json + "\n")
        .with_context(|| format!("failed to write {}", path.display()))?;

    Ok(())
}

fn build_settings(cwd: &Path) -> Value {
    let rust_checks_path = cwd.join(".claude/hooks/post-edit-rust-checks.sh");
    let include_rust_checks = rust_checks_path.exists();

    let post_edit_hooks: Vec<Value> = {
        let mut hooks = vec![
            serde_json::json!({"type": "command", "command": "workflow-runner hook post-edit"}),
        ];
        if include_rust_checks {
            hooks.push(serde_json::json!({"type": "command", "command": ".claude/hooks/post-edit-rust-checks.sh"}));
        }
        hooks
    };

    serde_json::json!({
        "hooks": {
            "PostToolUse": [
                {"matcher": "Edit",  "hooks": post_edit_hooks.clone()},
                {"matcher": "Write", "hooks": post_edit_hooks}
            ],
            "PreToolUse": [
                {"matcher": "Edit",  "hooks": [{"type": "command", "command": "workflow-runner hook pre-edit"}]},
                {"matcher": "Write", "hooks": [{"type": "command", "command": "workflow-runner hook pre-edit"}]},
                {"matcher": "Bash",  "hooks": [{"type": "command", "command": "workflow-runner hook pre-bash"}]}
            ]
        }
    })
}

/// Merges target hook entries into existing settings, preserving non-workflow-runner hooks.
fn merge_hook_settings(mut existing: Value, target: Value) -> Value {
    let target_hooks = match target.pointer("/hooks").and_then(|h| h.as_object()) {
        Some(h) => h.clone(),
        None => return existing,
    };

    // Ensure hooks object exists.
    if existing.pointer("/hooks").is_none() {
        existing["hooks"] = serde_json::json!({});
    }

    for (hook_type, target_entries) in &target_hooks {
        let target_arr: Vec<Value> = match target_entries.as_array() {
            Some(a) => a.clone(),
            None => continue,
        };

        let pointer = format!("/hooks/{}", hook_type);
        let existing_arr: Vec<Value> = existing
            .pointer(&pointer)
            .and_then(|h| h.as_array())
            .cloned()
            .unwrap_or_default();

        let merged = merge_hook_type_array(existing_arr, target_arr);
        existing["hooks"][hook_type] = Value::Array(merged);
    }

    existing
}

/// Merges one hook-type array (e.g. PreToolUse entries).
/// For matchers covered by target: replaces wf-runner hooks, keeps non-wf-runner hooks.
/// For other matchers: keeps them unchanged.
fn merge_hook_type_array(existing: Vec<Value>, target: Vec<Value>) -> Vec<Value> {
    let target_matchers: HashSet<String> = target
        .iter()
        .filter_map(|e| e["matcher"].as_str().map(|s| s.to_string()))
        .collect();

    // Collect non-wf-runner hooks keyed by matcher from existing entries.
    let mut preserved_by_matcher: HashMap<String, Vec<Value>> = HashMap::new();
    for entry in &existing {
        if let Some(matcher) = entry["matcher"].as_str() {
            if target_matchers.contains(matcher) {
                let non_wf: Vec<Value> = entry["hooks"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|h| !is_wf_runner_hook(h))
                    .cloned()
                    .collect();
                preserved_by_matcher
                    .entry(matcher.to_string())
                    .or_default()
                    .extend(non_wf);
            }
        }
    }

    // Keep existing entries for matchers not covered by target.
    let mut merged: Vec<Value> = existing
        .into_iter()
        .filter(|e| {
            e["matcher"]
                .as_str()
                .map(|m| !target_matchers.contains(m))
                .unwrap_or(true)
        })
        .collect();

    // Append target entries, appending preserved non-wf-runner hooks after wf-runner hooks.
    for entry in target {
        let matcher = entry["matcher"].as_str().unwrap_or("");
        let wf_hooks: Vec<Value> = entry["hooks"].as_array().cloned().unwrap_or_default();
        let extra = preserved_by_matcher.remove(matcher).unwrap_or_default();
        let all_hooks: Vec<Value> = wf_hooks.into_iter().chain(extra).collect();
        merged.push(serde_json::json!({"matcher": matcher, "hooks": all_hooks}));
    }

    merged
}

fn is_wf_runner_hook(hook: &Value) -> bool {
    hook["command"]
        .as_str()
        .map(|c| c.starts_with(WORKFLOW_RUNNER_CMD_PREFIX))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn setup_dir_with_rust_checks(include_hook: bool) -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        let hooks_dir = dir.path().join(".claude/hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        if include_hook {
            std::fs::write(hooks_dir.join("post-edit-rust-checks.sh"), "#!/bin/sh\n").unwrap();
        }
        dir
    }

    #[test]
    fn write_creates_settings_json() {
        let dir = setup_dir_with_rust_checks(false);
        write_settings_json(dir.path()).unwrap();

        let content = std::fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap();
        let v: Value = serde_json::from_str(&content).unwrap();

        // PreToolUse should have Edit, Write, Bash matchers.
        let pre = v["hooks"]["PreToolUse"].as_array().unwrap();
        let matchers: Vec<&str> = pre.iter().filter_map(|e| e["matcher"].as_str()).collect();
        assert!(!matchers.contains(&"TaskUpdate"));
        assert!(matchers.contains(&"Edit"));
        assert!(matchers.contains(&"Write"));
        assert!(matchers.contains(&"Bash"));

        // PostToolUse should have Edit and Write.
        let post = v["hooks"]["PostToolUse"].as_array().unwrap();
        let matchers: Vec<&str> = post.iter().filter_map(|e| e["matcher"].as_str()).collect();
        assert!(matchers.contains(&"Edit"));
        assert!(matchers.contains(&"Write"));
    }

    #[test]
    fn write_includes_rust_checks_when_file_exists() {
        let dir = setup_dir_with_rust_checks(true);
        write_settings_json(dir.path()).unwrap();

        let content = std::fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap();
        assert!(content.contains("post-edit-rust-checks.sh"));
    }

    #[test]
    fn write_excludes_rust_checks_when_file_absent() {
        let dir = setup_dir_with_rust_checks(false);
        write_settings_json(dir.path()).unwrap();

        let content = std::fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap();
        assert!(!content.contains("post-edit-rust-checks.sh"));
    }

    #[test]
    fn merge_preserves_non_wf_runner_hooks() {
        let dir = setup_dir_with_rust_checks(false);
        let path = dir.path().join(".claude/settings.json");

        // Existing settings with a custom hook for Bash in PostToolUse.
        let existing = serde_json::json!({
            "hooks": {
                "PostToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [{"type": "command", "command": "my-custom-hook.sh"}]
                    }
                ]
            }
        });
        std::fs::write(&path, serde_json::to_string_pretty(&existing).unwrap()).unwrap();

        merge_settings_json(dir.path()).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let v: Value = serde_json::from_str(&content).unwrap();

        // Custom Bash hook in PostToolUse should be preserved (not a target matcher for PostToolUse).
        let post = v["hooks"]["PostToolUse"].as_array().unwrap();
        let bash_entry = post
            .iter()
            .find(|e| e["matcher"].as_str() == Some("Bash"))
            .unwrap();
        let bash_hooks = bash_entry["hooks"].as_array().unwrap();
        assert!(bash_hooks
            .iter()
            .any(|h| h["command"].as_str() == Some("my-custom-hook.sh")));

        // TaskUpdate matcher should not appear in PreToolUse.
        let pre = v["hooks"]["PreToolUse"].as_array().unwrap();
        assert!(pre
            .iter()
            .all(|e| e["matcher"].as_str() != Some("TaskUpdate")));
    }

    #[test]
    fn merge_replaces_old_wf_runner_hooks() {
        let dir = setup_dir_with_rust_checks(false);
        let path = dir.path().join(".claude/settings.json");

        // Simulate outdated wf-runner hooks for Edit matcher.
        let existing = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Edit",
                        "hooks": [{"type": "command", "command": "workflow-runner hook old-event"}]
                    }
                ]
            }
        });
        std::fs::write(&path, serde_json::to_string_pretty(&existing).unwrap()).unwrap();

        merge_settings_json(dir.path()).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let v: Value = serde_json::from_str(&content).unwrap();

        let pre = v["hooks"]["PreToolUse"].as_array().unwrap();
        let edit = pre
            .iter()
            .find(|e| e["matcher"].as_str() == Some("Edit"))
            .unwrap();
        let commands: Vec<&str> = edit["hooks"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|h| h["command"].as_str())
            .collect();

        assert!(commands.contains(&"workflow-runner hook pre-edit"));
        assert!(!commands.contains(&"workflow-runner hook old-event"));
    }
}
