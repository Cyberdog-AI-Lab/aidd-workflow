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
use adapters::standalone::channels as standalone_channels;
use adapters::standalone::runner as standalone_runner;
use config::loader::{load_config, validate as validate_config};
use engine::state::{ActionReport, StepStatus, WorkflowState};
use engine::store::{clear_state_by_id, load_state, load_state_by_id, save_state};
use engine::{dag, executor, gate};
use protocol::{
    input::ReportInput,
    output::{
        build_status, format_status_table, format_validate_text, CompleteOutput, ErrorOutput,
        FlowStatus, ValidateOutput, WorkflowListItem,
    },
};

#[derive(Parser)]
#[command(
    name = "workflow-runner",
    about = "Workflow execution engine for AI tools"
)]
struct Cli {
    /// Adapter name (claude-code | standalone)
    #[arg(long, default_value = "claude-code")]
    adapter: String,

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
    /// Start a workflow and return the first set of actions.
    Start { workflow: String },
    /// Return the next set of actions from the current state.
    Next,
    /// Record an action execution result (stdin: JSON).
    Report,
    /// Mark a step as complete (with gate check).
    Complete { step_id: String },
    /// Return resume information for an interrupted workflow.
    Resume,
    /// Return the current execution state as JSON.
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
        /// Event type: post-bash | pre-taskupdate | post-edit
        event_type: String,
    },
    /// Execute a step's actions directly (standalone adapter only).
    ExecStep { step_id: String },
    /// Initialize .workflow/ directory and generate .claude/settings.json.
    Init,
    /// Update .claude/settings.json with workflow-runner hooks (preserving existing entries).
    Update,
}

fn main() {
    let cli = Cli::parse();
    let cwd = cli.cwd.unwrap_or_else(|| std::env::current_dir().unwrap());

    let result = run(cli.command, &cwd, &cli.adapter, cli.workflow_id.as_deref());
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

fn run(cmd: Commands, cwd: &Path, adapter: &str, workflow_id: Option<&str>) -> Result<String> {
    match cmd {
        Commands::Start { workflow } => cmd_start(cwd, &workflow),
        Commands::Next => cmd_next(cwd, workflow_id),
        Commands::Report => cmd_report(cwd, workflow_id),
        Commands::Complete { step_id } => cmd_complete(cwd, &step_id, workflow_id),
        Commands::Resume => cmd_resume(cwd, workflow_id),
        Commands::Status { format } => cmd_status(cwd, &format, workflow_id),
        Commands::Validate { format } => cmd_validate(cwd, &format),
        Commands::List => cmd_list(cwd),
        Commands::Hook { event_type } => cmd_hook(cwd, &event_type, adapter),
        Commands::ExecStep { step_id } => cmd_exec_step(cwd, &step_id, workflow_id),
        Commands::Init => cmd_init(cwd),
        Commands::Update => cmd_update(cwd),
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

    {
        let s = state.steps.entry(input.step_id.clone()).or_default();
        if s.status == StepStatus::Pending {
            s.status = StepStatus::InProgress;
            s.started_at = Some(Utc::now());
        }
    }

    {
        let s = state.steps.entry(input.step_id.clone()).or_default();
        s.action_reports.push(ActionReport {
            action_index: input.action_index,
            action_type: input.action_type.clone(),
            exit_code: input.exit_code,
            stdout: input.stdout.clone(),
            recorded_at: Utc::now(),
        });
    }

    if let Some(parent_id) = dag::parent_of(&input.step_id) {
        state.sync_parallel_parent(parent_id, wf)?;
    }

    save_state(cwd, &state)?;

    let out = serde_json::json!({ "ok": true, "step_id": input.step_id });
    Ok(out.to_string())
}

fn cmd_complete(cwd: &Path, step_id: &str, workflow_id: Option<&str>) -> Result<String> {
    let config = load_config(cwd)?;
    let mut state = resolve_state(cwd, workflow_id)?.context("no workflow in progress")?;
    let wf = config
        .workflows
        .get(&state.workflow)
        .with_context(|| format!("workflow '{}' not found", state.workflow))?;

    // Run post_commands as the gate before allowing Complete.
    if let Some(step) = wf.steps.iter().find(|s| s.id == step_id) {
        if !step.post_commands.is_empty() {
            let already_recorded = state
                .steps
                .get(step_id)
                .map(|s| s.gate_recorded)
                .unwrap_or(false);

            if !already_recorded {
                let resolved: Vec<String> = step
                    .post_commands
                    .iter()
                    .map(|c| executor::resolve_template(c, &config))
                    .collect();

                for cmd in &resolved {
                    let result = standalone_runner::run_command(cmd, cwd)?;
                    if result.exit_code != 0 {
                        let output = CompleteOutput {
                            step_id: step_id.to_string(),
                            allowed: false,
                            reason: Some(format!(
                                "post_commands gate failed: '{}' exited with code {}",
                                cmd, result.exit_code
                            )),
                            next: None,
                        };
                        return Ok(serde_json::to_string_pretty(&output)?);
                    }
                }

                let s = state.steps.entry(step_id.to_string()).or_default();
                s.gate_recorded = true;
            }
        }
    }

    let gate_result = gate::check(wf, &state, step_id, cwd);
    if !gate_result.allowed {
        let output = CompleteOutput {
            step_id: step_id.to_string(),
            allowed: false,
            reason: gate_result.reason,
            next: None,
        };
        return Ok(serde_json::to_string_pretty(&output)?);
    }

    {
        let s = state.steps.entry(step_id.to_string()).or_default();
        s.status = StepStatus::Completed;
        s.completed_at = Some(Utc::now());
    }

    if let Some(parent_id) = dag::parent_of(step_id) {
        state.sync_parallel_parent(parent_id, wf)?;
    }

    save_state(cwd, &state)?;

    let next = executor::build_next(wf, &state, &config);
    if matches!(next.status, FlowStatus::Completed) {
        clear_state_by_id(cwd, &state.workflow_id)?;
    }

    let output = CompleteOutput {
        step_id: step_id.to_string(),
        allowed: true,
        reason: None,
        next: Some(next),
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

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => {
            let out = ValidateOutput {
                valid: false,
                workflow_count: 0,
                commands: vec![],
                errors: vec![format!(
                    ".workflow/config.yml not found: {}",
                    path.display()
                )],
            };
            return Ok(render_validate(&out, format));
        }
    };

    let config: crate::config::types::Config = match serde_yaml::from_str(&content) {
        Ok(c) => c,
        Err(e) => {
            let out = ValidateOutput {
                valid: false,
                workflow_count: 0,
                commands: vec![],
                errors: vec![format!("YAML parse error: {}", e)],
            };
            return Ok(render_validate(&out, format));
        }
    };

    let out = match validate_config(&config) {
        Err(ve) => ValidateOutput {
            valid: false,
            workflow_count: config.workflows.len(),
            commands: config.commands.keys().cloned().collect(),
            errors: ve.errors,
        },
        Ok(()) => {
            let mut commands: Vec<String> = config.commands.keys().cloned().collect();
            commands.sort();
            ValidateOutput {
                valid: true,
                workflow_count: config.workflows.len(),
                commands,
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
            step_count: wf.steps.len(),
        })
        .collect();
    items.sort_by(|a, b| a.slug.cmp(&b.slug));
    Ok(serde_json::to_string_pretty(&items)?)
}

fn cmd_hook(cwd: &Path, event_type: &str, _adapter: &str) -> Result<String> {
    let input = read_stdin().unwrap_or_default();
    let effective_cwd = extract_cwd_from_stdin(&input, cwd);

    // Hook errors must not crash the process; return empty on failure.
    let result: Option<String> = match event_type {
        "post-bash" => {
            let _ = hook_handler::handle_post_bash(&effective_cwd, &input);
            None
        }
        "pre-taskupdate" => {
            hook_handler::handle_pre_taskupdate(&effective_cwd, &input).unwrap_or(None)
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

fn cmd_exec_step(cwd: &Path, step_id: &str, workflow_id: Option<&str>) -> Result<String> {
    use crate::config::types::Action;

    let config = load_config(cwd)?;
    let state = resolve_state(cwd, workflow_id)?.context("no workflow in progress")?;
    let wf = config
        .workflows
        .get(&state.workflow)
        .with_context(|| format!("workflow '{}' not found", state.workflow))?;

    let step = wf
        .steps
        .iter()
        .find(|s| s.id == step_id)
        .with_context(|| format!("step '{}' not found in workflow", step_id))?;

    let wf_id = state.workflow_id.clone();

    // Run pre_commands before the step body.
    for cmd in &step.pre_commands {
        let resolved = executor::resolve_template(cmd, &config);
        let result = standalone_runner::run_command(&resolved, cwd)?;
        eprintln!("{}", result.stderr.trim_end());
        if result.exit_code != 0 {
            anyhow::bail!(
                "pre_commands failed: '{}' exited with code {}",
                resolved,
                result.exit_code
            );
        }
    }

    for (idx, action) in step.actions.iter().enumerate() {
        let (exit_code, stdout, _stderr) = match action {
            Action::Agent { prompt, .. } => {
                let result = standalone_channels::run_agent(prompt, cwd)?;
                println!("{}", result.stdout);
                (0, result.stdout, String::new())
            }
            Action::Skill { skill, .. } => {
                anyhow::bail!(
                    "skill action '{}' is not supported in standalone mode",
                    skill
                )
            }
            Action::Workflow { workflow, .. } => {
                anyhow::bail!(
                    "workflow action '{}' is not supported in standalone mode",
                    workflow
                )
            }
        };

        {
            use chrono::Utc;
            use engine::state::{ActionReport, StepStatus};
            let mut st = resolve_state(cwd, Some(&wf_id))?.context("no workflow in progress")?;
            {
                let s = st.steps.entry(step_id.to_string()).or_default();
                if s.status == StepStatus::Pending {
                    s.status = StepStatus::InProgress;
                    s.started_at = Some(Utc::now());
                }
                s.action_reports.push(ActionReport {
                    action_index: idx,
                    action_type: action_type_name(action).to_string(),
                    exit_code: Some(exit_code),
                    stdout: Some(stdout.clone()),
                    recorded_at: Utc::now(),
                });
            }
            save_state(cwd, &st)?;
        }

        if exit_code != 0 {
            anyhow::bail!(
                "action exited with code {}; aborting step '{}'",
                exit_code,
                step_id
            );
        }
    }

    // cmd_complete handles post_commands gate automatically.
    cmd_complete(cwd, step_id, Some(&wf_id))
}

fn cmd_init(cwd: &Path) -> Result<String> {
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

    infra::settings_writer::write_settings_json(cwd)?;

    let out = serde_json::json!({
        "ok": true,
        "message": "initialized: .workflow/ and .claude/settings.json created"
    });
    Ok(serde_json::to_string_pretty(&out)?)
}

fn cmd_update(cwd: &Path) -> Result<String> {
    infra::settings_writer::merge_settings_json(cwd)?;

    let out = serde_json::json!({
        "ok": true,
        "message": "updated: .claude/settings.json merged with workflow-runner hooks"
    });
    Ok(serde_json::to_string_pretty(&out)?)
}

fn action_type_name(action: &crate::config::types::Action) -> &'static str {
    use crate::config::types::Action;
    match action {
        Action::Agent { .. } => "agent",
        Action::Skill { .. } => "skill",
        Action::Workflow { .. } => "workflow",
    }
}
