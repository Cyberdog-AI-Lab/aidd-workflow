mod adapters;
mod config;
mod engine;
mod infra;
mod protocol;
mod providers;

use anyhow::{Context, Result};
use chrono::Utc;
use clap::{Parser, Subcommand};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use adapters::hooks::hook_handler;
use config::loader::{load_and_merge_config, load_config, validate as validate_config};
use engine::state::{ActionReport, StepStatus, WorkflowState};
use engine::store::{
    clear_state_by_id, get_workflow_status, load_state, load_state_by_id, save_state,
    set_workflow_status,
};
use engine::{dag, executor, gate};
use protocol::{
    input::ReportInput,
    output::{
        build_status, format_status_table, format_validate_text, CompleteOutput, ErrorOutput,
        FlowStatus, RejectOutput, ValidateOutput, WorkflowListItem, WorkflowOutput,
    },
};

#[derive(Parser)]
#[command(
    name = "workflow-runner",
    about = "Workflow execution engine for AI tools"
)]
struct Cli {
    /// Project root directory (defaults to current directory)
    #[arg(long)]
    cwd: Option<PathBuf>,

    /// Workflow ID to target (required when multiple active workflows exist)
    #[arg(long)]
    workflow_id: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start a workflow and return the first set of tasks.
    Start { workflow: String },
    /// Return the next set of tasks. When awaiting approval, calling this approves and proceeds.
    Next,
    /// Record a task execution result (stdin: JSON).
    Report,
    /// Mark a task as complete (with gate check).
    Complete { task_id: String },
    /// Reject an awaiting-approval task and retry it.
    Reject {
        task_id: String,
        /// Developer feedback explaining the rejection.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Return resume information for an interrupted workflow.
    Resume,
    /// Return the current execution state.
    Status {
        /// Output format: json (default) or table
        #[arg(long, default_value = "json", value_parser = ["json", "table"])]
        format: String,
    },
    /// Validate config.yml.
    Validate {
        /// Output format: json (default) or text
        #[arg(long, default_value = "json", value_parser = ["json", "text"])]
        format: String,
    },
    /// List available workflows.
    List,
    /// Process a Claude Code hook event (stdin: hook JSON).
    Hook {
        /// Event type: post-bash | post-edit | pre-edit | pre-bash
        event_type: String,
    },
    /// Set up .workflow/ directory and .claude/settings.json with workflow-runner hooks
    /// (preserving existing entries).
    Setup,
    /// Print the JSON Schema for config.yml to stdout.
    DumpSchema,
}

fn main() {
    let cli = Cli::parse();
    let cwd = cli.cwd.unwrap_or_else(|| std::env::current_dir().unwrap());

    let result = run(cli.command, &cwd, cli.workflow_id.as_deref());
    match result {
        Ok(json) => {
            if !json.is_empty() {
                println!("{}", json);
            }
        }
        Err(e) => {
            let out = serde_json::to_string(&ErrorOutput {
                error: e.to_string(),
            })
            .unwrap();
            eprintln!("{}", out);
            std::process::exit(1);
        }
    }
}

fn run(cmd: Commands, cwd: &Path, workflow_id: Option<&str>) -> Result<String> {
    match cmd {
        Commands::Start { workflow } => cmd_start(cwd, &workflow),
        Commands::Next => cmd_next(cwd, workflow_id),
        Commands::Report => cmd_report(cwd, workflow_id),
        Commands::Complete { task_id } => cmd_complete(cwd, &task_id, workflow_id),
        Commands::Reject { task_id, reason } => {
            cmd_reject(cwd, &task_id, reason.as_deref(), workflow_id)
        }
        Commands::Resume => cmd_resume(cwd, workflow_id),
        Commands::Status { format } => cmd_status(cwd, &format, workflow_id),
        Commands::Validate { format } => cmd_validate(cwd, &format),
        Commands::List => cmd_list(cwd),
        Commands::Hook { event_type } => cmd_hook(cwd, &event_type),
        Commands::Setup => cmd_setup(cwd),
        Commands::DumpSchema => cmd_dump_schema(),
    }
}

fn resolve_state(cwd: &Path, workflow_id: Option<&str>) -> Result<Option<WorkflowState>> {
    match workflow_id {
        Some(id) => load_state_by_id(cwd, id),
        None => load_state(cwd),
    }
}

fn cmd_start(cwd: &Path, workflow_name: &str) -> Result<String> {
    let config = load_config(cwd)?;
    let wf = config
        .workflows
        .get(workflow_name)
        .with_context(|| format!("workflow '{}' not found in config.yml", workflow_name))?;

    let state = WorkflowState::new(workflow_name, wf);
    save_state(cwd, &state)?;

    let mut output = executor::build_next(wf, &state, &config);
    if matches!(output.status, FlowStatus::InProgress) {
        output.status = FlowStatus::Started;
    }
    Ok(serde_json::to_string_pretty(&output)?)
}

fn cmd_next(cwd: &Path, workflow_id: Option<&str>) -> Result<String> {
    let config = load_config(cwd)?;
    let state = resolve_state(cwd, workflow_id)?
        .context("no workflow in progress; run `workflow-runner start <workflow>` first")?;
    let wf = config
        .workflows
        .get(&state.workflow)
        .with_context(|| format!("workflow '{}' not found in config.yml", state.workflow))?;

    // When awaiting approval, calling `next` acts as approval: clear the gate and proceed.
    let wf_status = get_workflow_status(cwd, &state.workflow_id)?;
    if wf_status == "awaiting_approval" {
        set_workflow_status(cwd, &state.workflow_id, "active")?;
    }

    let output = executor::build_next(wf, &state, &config);
    Ok(serde_json::to_string_pretty(&output)?)
}

fn cmd_report(cwd: &Path, workflow_id: Option<&str>) -> Result<String> {
    let input_str = read_stdin()?;
    let input: ReportInput =
        serde_json::from_str(&input_str).context("report stdin is not valid JSON")?;

    let config = load_config(cwd)?;
    let mut state = resolve_state(cwd, workflow_id)?.context("no workflow in progress")?;

    let wf = config
        .workflows
        .get(&state.workflow)
        .with_context(|| format!("workflow '{}' not found", state.workflow))?;

    // Reject reports for task IDs that were never initialised in the workflow state.
    // state.tasks is pre-populated from wf.tasks by WorkflowState::new, so an unknown
    // ID means either a typo or a stale task from a different workflow.
    if !state.tasks.contains_key(&input.task_id) {
        anyhow::bail!(
            "unknown task_id '{}' for workflow '{}'",
            input.task_id,
            state.workflow
        );
    }

    {
        let s = state.tasks.entry(input.task_id.clone()).or_default();
        if s.status == StepStatus::Pending {
            s.status = StepStatus::InProgress;
            s.started_at = Some(Utc::now());
        }
    }

    {
        let s = state.tasks.entry(input.task_id.clone()).or_default();
        s.action_reports.push(ActionReport {
            action_index: input.action_index,
            action_type: input.action_type.clone(),
            exit_code: input.exit_code,
            stdout: input.stdout.clone(),
            recorded_at: Utc::now(),
        });
    }

    if let Some(parent_id) = dag::parent_of(&input.task_id) {
        state.sync_agents_parent(parent_id, wf)?;
    }

    save_state(cwd, &state)?;

    let out = serde_json::json!({ "ok": true, "task_id": input.task_id });
    Ok(out.to_string())
}

fn cmd_complete(cwd: &Path, task_id: &str, workflow_id: Option<&str>) -> Result<String> {
    let config = load_config(cwd)?;
    let mut state = resolve_state(cwd, workflow_id)?.context("no workflow in progress")?;
    let wf = config
        .workflows
        .get(&state.workflow)
        .with_context(|| format!("workflow '{}' not found", state.workflow))?;

    let gate_result = gate::check(wf, &state, task_id);
    if !gate_result.allowed {
        let output = CompleteOutput {
            task_id: task_id.to_string(),
            allowed: false,
            reason: gate_result.reason,
            next: None,
        };
        return Ok(serde_json::to_string_pretty(&output)?);
    }

    {
        let s = state.tasks.entry(task_id.to_string()).or_default();
        s.status = StepStatus::Completed;
        s.completed_at = Some(Utc::now());
    }

    if let Some(parent_id) = dag::parent_of(task_id) {
        state.sync_agents_parent(parent_id, wf)?;
    }

    save_state(cwd, &state)?;

    // For parent tasks (not sub-agents): check approval flag.
    if dag::parent_of(task_id).is_none() {
        if let Some(task_cfg) = wf.tasks.iter().find(|t| t.id == task_id) {
            if task_cfg.approval {
                set_workflow_status(cwd, &state.workflow_id, "awaiting_approval")?;
                let output = CompleteOutput {
                    task_id: task_id.to_string(),
                    allowed: true,
                    reason: None,
                    next: Some(WorkflowOutput {
                        workflow_id: state.workflow_id.clone(),
                        workflow: state.workflow.clone(),
                        status: FlowStatus::AwaitingApproval,
                        tasks: vec![],
                    }),
                };
                return Ok(serde_json::to_string_pretty(&output)?);
            }
        }
    }

    let next = executor::build_next(wf, &state, &config);
    if matches!(next.status, FlowStatus::Completed) {
        clear_state_by_id(cwd, &state.workflow_id)?;
    }

    let output = CompleteOutput {
        task_id: task_id.to_string(),
        allowed: true,
        reason: None,
        next: Some(next),
    };
    Ok(serde_json::to_string_pretty(&output)?)
}

fn cmd_reject(
    cwd: &Path,
    task_id: &str,
    reason: Option<&str>,
    workflow_id: Option<&str>,
) -> Result<String> {
    let config = load_config(cwd)?;
    let mut state = resolve_state(cwd, workflow_id)?
        .context("no workflow in progress (or not awaiting approval)")?;
    let wf = config
        .workflows
        .get(&state.workflow)
        .with_context(|| format!("workflow '{}' not found", state.workflow))?;

    let wf_status = get_workflow_status(cwd, &state.workflow_id)?;
    if wf_status != "awaiting_approval" {
        anyhow::bail!(
            "workflow is not awaiting approval (current status: {}); nothing to reject",
            wf_status
        );
    }

    // Validate that the given task_id exists in the workflow config.
    if !wf.tasks.iter().any(|t| t.id == task_id) {
        anyhow::bail!(
            "task '{}' not found in workflow '{}'",
            task_id,
            state.workflow
        );
    }
    // Only Completed tasks can be rejected (the approval gate fires after completion).
    if !matches!(
        state.tasks.get(task_id).map(|s| &s.status),
        Some(StepStatus::Completed)
    ) {
        anyhow::bail!(
            "task '{}' is not in Completed state; cannot reject",
            task_id
        );
    }

    // Reset task to InProgress and record rejection reason.
    {
        let s = state.tasks.entry(task_id.to_string()).or_default();
        s.status = StepStatus::InProgress;
        s.completed_at = None;
        if let Some(reason_str) = reason {
            s.action_reports.push(ActionReport {
                action_index: s.action_reports.len(),
                action_type: "reject".to_string(),
                exit_code: None,
                stdout: Some(reason_str.to_string()),
                recorded_at: Utc::now(),
            });
        }
    }

    // Clear the approval gate.
    set_workflow_status(cwd, &state.workflow_id, "active")?;
    save_state(cwd, &state)?;

    // Build the task output for re-dispatch.
    let task_output =
        wf.tasks
            .iter()
            .find(|t| t.id == task_id)
            .map(|task| protocol::output::TaskOutput {
                task_id: task.id.clone(),
                task: task.task.clone(),
                prompt: task
                    .prompt
                    .as_deref()
                    .map(|p| executor::resolve_template(p, &config)),
                skills: task.skills.clone(),
                agents: task.agents.clone(),
                outputs: task.outputs.clone(),
                deny: task.deny.clone(),
                approval: task.approval,
            });

    let output = RejectOutput {
        task_id: task_id.to_string(),
        reason: reason.map(str::to_string),
        task: task_output,
    };
    Ok(serde_json::to_string_pretty(&output)?)
}

fn cmd_resume(cwd: &Path, workflow_id: Option<&str>) -> Result<String> {
    let config = load_config(cwd)?;
    let state = resolve_state(cwd, workflow_id)?.context("no workflow in progress")?;
    let wf = config
        .workflows
        .get(&state.workflow)
        .with_context(|| format!("workflow '{}' not found", state.workflow))?;

    let output = executor::build_next(wf, &state, &config);
    Ok(serde_json::to_string_pretty(&output)?)
}

fn cmd_status(cwd: &Path, format: &str, workflow_id: Option<&str>) -> Result<String> {
    let config = load_config(cwd)?;
    let state = resolve_state(cwd, workflow_id)?.context("no workflow in progress")?;
    let wf = config
        .workflows
        .get(&state.workflow)
        .with_context(|| format!("workflow '{}' not found", state.workflow))?;

    let status = build_status(&state, wf);
    if format == "table" {
        Ok(format_status_table(&status))
    } else {
        Ok(serde_json::to_string_pretty(&status)?)
    }
}

fn cmd_validate(cwd: &Path, format: &str) -> Result<String> {
    let path = cwd.join(".workflow/config.yml");

    if !path.exists() {
        let out = ValidateOutput {
            valid: false,
            workflow_count: 0,
            vars: vec![],
            errors: vec![format!(
                ".workflow/config.yml not found: {}",
                path.display()
            )],
        };
        return Ok(render_validate(&out, format));
    }

    // Use load_and_merge_config so that imported files are included before validation.
    // This prevents false positives/negatives from cross-import requires references.
    let config = match load_and_merge_config(cwd) {
        Ok(c) => c,
        Err(e) => {
            let out = ValidateOutput {
                valid: false,
                workflow_count: 0,
                vars: vec![],
                errors: vec![e.to_string()],
            };
            return Ok(render_validate(&out, format));
        }
    };

    let out = match validate_config(&config) {
        Err(ve) => ValidateOutput {
            valid: false,
            workflow_count: config.workflows.len(),
            vars: config.vars.keys().cloned().collect(),
            errors: ve.errors,
        },
        Ok(()) => {
            let mut vars: Vec<String> = config.vars.keys().cloned().collect();
            vars.sort();
            ValidateOutput {
                valid: true,
                workflow_count: config.workflows.len(),
                vars,
                errors: vec![],
            }
        }
    };

    Ok(render_validate(&out, format))
}

fn render_validate(out: &ValidateOutput, format: &str) -> String {
    if format == "text" {
        format_validate_text(out)
    } else {
        serde_json::to_string_pretty(out).unwrap()
    }
}

fn cmd_list(cwd: &Path) -> Result<String> {
    let config = load_config(cwd)?;
    let mut items: Vec<WorkflowListItem> = config
        .workflows
        .iter()
        .map(|(slug, wf)| WorkflowListItem {
            slug: slug.clone(),
            name: wf.name.clone(),
            description: wf.description.clone(),
            task_count: wf.tasks.len(),
        })
        .collect();
    items.sort_by(|a, b| a.slug.cmp(&b.slug));
    Ok(serde_json::to_string_pretty(&items)?)
}

fn cmd_hook(cwd: &Path, event_type: &str) -> Result<String> {
    let input = read_stdin().unwrap_or_default();
    let effective_cwd = extract_cwd_from_stdin(&input, cwd);

    // Hook errors must not crash the process; return empty on failure.
    let result: Option<String> = match event_type {
        "post-bash" => {
            let _ = hook_handler::handle_post_bash(&effective_cwd, &input);
            None
        }
        "post-edit" => hook_handler::handle_post_edit(&effective_cwd, &input).unwrap_or(None),
        "pre-edit" => hook_handler::handle_pre_edit(&effective_cwd, &input).unwrap_or(None),
        "pre-bash" => hook_handler::handle_pre_bash(&effective_cwd, &input).unwrap_or(None),
        _ => None,
    };

    Ok(result.unwrap_or_default())
}

fn extract_cwd_from_stdin(stdin_str: &str, fallback: &Path) -> PathBuf {
    let v: serde_json::Value = serde_json::from_str(stdin_str).unwrap_or_default();
    v["cwd"]
        .as_str()
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .unwrap_or_else(|| fallback.to_path_buf())
}

fn read_stdin() -> Result<String> {
    let mut buf = String::new();
    io::stdin().read_to_string(&mut buf)?;
    Ok(buf)
}

fn cmd_setup(cwd: &Path) -> Result<String> {
    let workflow_dir = cwd.join(".workflow");
    std::fs::create_dir_all(&workflow_dir).context("failed to create .workflow directory")?;

    let in_path = std::process::Command::new("which")
        .arg("workflow-runner")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !in_path {
        eprintln!("warning: workflow-runner not found in PATH; ensure it is installed");
    }

    infra::settings_writer::merge_settings_json(cwd)?;

    let out = serde_json::json!({
        "ok": true,
        "message": "set up: .workflow/ created and .claude/settings.json merged with workflow-runner hooks"
    });
    Ok(serde_json::to_string_pretty(&out)?)
}

fn cmd_dump_schema() -> Result<String> {
    let schema = schemars::schema_for!(config::types::Config);
    Ok(serde_json::to_string_pretty(&schema)?)
}
