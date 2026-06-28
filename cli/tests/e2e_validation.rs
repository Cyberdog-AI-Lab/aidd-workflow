//! E2E tests for Scenario 6: validate / list / dump-schema.
//!
//! Covers:
//!   6-1  validate on a valid config returns valid:true with correct counts
//!   6-2  validate reports duplicate task IDs
//!   6-3  validate reports undefined requires references
//!   6-4  validate when config.yml is missing returns valid:false
//!   6-5  validate --format text produces human-readable output
//!   6-6  list returns all workflows sorted by slug
//!   6-7  dump-schema produces valid JSON containing a $schema field
//!   6-8  dump-schema output matches the committed workflow.schema.json

mod helpers;
use helpers::{TempProject, CONFIG_STANDARD};

/// Config with a duplicate task ID — triggers validation error.
const CONFIG_DUPLICATE_IDS: &str = r#"
workflows:
  broken:
    name: Broken
    tasks:
      - id: step
        task: First step
        prompt: Do the first step.
      - id: step
        task: Duplicate
        prompt: This id already exists.
"#;

/// Config with an undefined requires reference.
const CONFIG_UNDEFINED_REQUIRES: &str = r#"
workflows:
  broken:
    name: Broken
    tasks:
      - id: step
        task: Step
        prompt: Do the step.
        requires: [ghost]
"#;

// ── 6-1 ───────────────────────────────────────────────────────────────────────

/// A well-formed config must produce `valid: true` with the correct workflow
/// count and var keys.
#[test]
fn validate_valid_config_returns_true() {
    let proj = TempProject::new(CONFIG_STANDARD);
    let out = proj.validate();

    assert_eq!(out["valid"], true);
    // CONFIG_STANDARD defines three workflows.
    assert_eq!(out["workflow_count"], 3);

    let vars = out["vars"].as_array().expect("vars must be an array");
    let var_names: Vec<&str> = vars.iter().map(|v| v.as_str().unwrap()).collect();
    // Vars are returned sorted.
    assert!(var_names.contains(&"test"));
    assert!(var_names.contains(&"lint"));
    assert!(var_names.contains(&"build"));

    let errors = out["errors"].as_array().expect("errors must be an array");
    assert!(errors.is_empty(), "no errors expected for a valid config");
}

// ── 6-2 ───────────────────────────────────────────────────────────────────────

/// Duplicate task IDs must be reported in the errors list.
/// `validate` always exits 0; the invalid state is communicated via `valid: false`.
#[test]
fn validate_reports_duplicate_task_ids() {
    let proj = TempProject::new(CONFIG_DUPLICATE_IDS);
    let out = proj.validate();

    assert_eq!(out["valid"], false);
    let errors = out["errors"].as_array().expect("errors must be an array");
    assert!(
        !errors.is_empty(),
        "duplicate task ID must produce at least one error"
    );

    let combined: String = errors.iter().map(|e| e.as_str().unwrap_or("")).collect();
    assert!(
        combined.contains("step"),
        "error must mention the duplicate task ID 'step': got '{combined}'"
    );
}

// ── 6-3 ───────────────────────────────────────────────────────────────────────

/// A requires reference to a non-existent task must be flagged.
#[test]
fn validate_reports_undefined_requires() {
    let proj = TempProject::new(CONFIG_UNDEFINED_REQUIRES);
    let out = proj.validate();

    assert_eq!(out["valid"], false);
    let errors = out["errors"].as_array().expect("errors must be an array");
    let combined: String = errors.iter().map(|e| e.as_str().unwrap_or("")).collect();
    assert!(
        combined.contains("ghost"),
        "error must mention the undefined requires target 'ghost': got '{combined}'"
    );
}

// ── 6-4 ───────────────────────────────────────────────────────────────────────

/// When config.yml does not exist, `validate` must return `valid: false` with
/// a message that includes the expected file path.
#[test]
fn validate_missing_config_returns_false() {
    let proj = TempProject::empty();
    let out = proj.validate();

    assert_eq!(out["valid"], false);
    assert_eq!(out["workflow_count"], 0);

    let errors = out["errors"].as_array().expect("errors must be an array");
    let combined: String = errors.iter().map(|e| e.as_str().unwrap_or("")).collect();
    assert!(
        combined.contains("config.yml"),
        "error must mention config.yml: got '{combined}'"
    );
}

// ── 6-5 ───────────────────────────────────────────────────────────────────────

/// `validate --format text` must produce human-readable plain text, not JSON.
#[test]
fn validate_text_format_is_human_readable() {
    let proj = TempProject::new(CONFIG_STANDARD);

    // text format is not JSON — use raw run() to capture stdout.
    let output = proj.run(&["validate", "--format", "text"]);
    assert!(
        output.status.success(),
        "validate must exit 0 even in text mode"
    );

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("valid"),
        "text output must contain the word 'valid': got '{text}'"
    );
    assert!(
        text.contains("3"),
        "text output must mention the workflow count (3): got '{text}'"
    );
    // Plain text must NOT start with '{' (that would be JSON).
    assert!(
        !text.trim_start().starts_with('{'),
        "text format must not be JSON: got '{text}'"
    );
}

// ── 6-6 ───────────────────────────────────────────────────────────────────────

/// `list` must return all workflows with slug, name, description, and task_count.
/// Items must be sorted by slug.
#[test]
fn list_returns_all_workflows_sorted_by_slug() {
    let proj = TempProject::new(CONFIG_STANDARD);
    let out = proj.list();

    let items = out.as_array().expect("list must return a JSON array");
    assert_eq!(items.len(), 3, "CONFIG_STANDARD defines three workflows");

    // Verify sorted order: bug-fix < feature < release (lexicographic).
    assert_eq!(items[0]["slug"], "bug-fix");
    assert_eq!(items[1]["slug"], "feature");
    assert_eq!(items[2]["slug"], "release");

    // Each item must carry the expected fields.
    for item in items {
        assert!(
            item["slug"].as_str().is_some_and(|s| !s.is_empty()),
            "slug must be a non-empty string"
        );
        assert!(
            item["name"].as_str().is_some_and(|s| !s.is_empty()),
            "name must be a non-empty string"
        );
        assert!(
            item["task_count"].as_u64().is_some_and(|n| n > 0),
            "task_count must be a positive integer"
        );
    }

    // Spot-check task counts (CONFIG_STANDARD: bug-fix=4, feature=3, release=4).
    assert_eq!(items[0]["task_count"], 4, "bug-fix has 4 tasks");
    assert_eq!(items[1]["task_count"], 3, "feature has 3 tasks");
    assert_eq!(items[2]["task_count"], 4, "release has 4 tasks");
}

// ── 6-7 ───────────────────────────────────────────────────────────────────────

/// `dump-schema` must output valid JSON that looks like a JSON Schema document.
#[test]
fn dump_schema_returns_valid_json_schema() {
    // dump-schema has no --cwd dependency, but TempProject still works fine.
    let proj = TempProject::new(CONFIG_STANDARD);
    let output = proj.run(&["dump-schema"]);

    assert!(output.status.success(), "dump-schema must exit 0");

    let raw = String::from_utf8_lossy(&output.stdout);
    let schema: serde_json::Value =
        serde_json::from_str(raw.trim()).expect("dump-schema must produce valid JSON");

    // schemars generates a `$schema` field.
    assert!(
        schema["$schema"].as_str().is_some(),
        "schema must contain a $schema field"
    );
    // Top-level type must be an object (Config is a struct).
    assert!(
        schema["title"].as_str().is_some() || schema["properties"].is_object(),
        "schema must describe the Config struct"
    );
}

// ── 6-8 ───────────────────────────────────────────────────────────────────────

/// The `dump-schema` output must match the committed `.workflow/workflow.schema.json`.
/// This is the CLI-level counterpart of the `schema_file_matches_generated` unit test.
#[test]
fn dump_schema_matches_committed_schema_file() {
    let proj = TempProject::new(CONFIG_STANDARD);
    let output = proj.run(&["dump-schema"]);
    assert!(output.status.success());

    let generated = String::from_utf8_lossy(&output.stdout);
    let generated_val: serde_json::Value =
        serde_json::from_str(generated.trim()).expect("generated schema must be valid JSON");

    // Read the committed schema relative to the Cargo workspace root.
    let schema_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".workflow/workflow.schema.json");
    let committed_raw =
        std::fs::read_to_string(&schema_path).expect("workflow.schema.json must exist");
    let committed_val: serde_json::Value =
        serde_json::from_str(&committed_raw).expect("committed schema must be valid JSON");

    assert_eq!(
        generated_val, committed_val,
        "dump-schema output must match .workflow/workflow.schema.json; \
         run `workflow-runner dump-schema > .workflow/workflow.schema.json` to update it"
    );
}
