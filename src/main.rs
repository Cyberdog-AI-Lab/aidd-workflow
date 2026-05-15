mod adapters;
mod config;
mod engine;
mod protocol;

use anyhow::{Context, Result};
use chrono::Utc;
use clap::{Parser, Subcommand};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use adapters::claude_code::hook_handler;
use adapters::standalone::runner as standalone_runner;
use config::loader::load_config;
use engine::state::{ActionReport, StepStatus, WorkflowState};
use engine::store::{clear_state, load_state, save_state};
use engine::{dag, executor, gate};
use protocol::{
    input::ReportInput,
    output::{build_status, CompleteOutput, ErrorOutput, FlowStatus, WorkflowListItem},
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
    Status,
    /// Validate config.yml.
    Validate,
    /// List available workflows.
    List,
    /// Process a Claude Code hook event (stdin: hook JSON).
    Hook {
        /// Event type: post-bash | pre-taskupdate | post-edit
        event_type: String,
    },
    /// Execute a step's actions directly (standalone adapter only).
    ExecStep { step_id: String },
}

fn main() {
    let cli = Cli::parse();
    let cwd = cli.cwd.unwrap_or_else(|| std::env::current_dir().unwrap());

    let result = run(cli.command, &cwd, &cli.adapter);
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

fn run(cmd: Commands, cwd: &Path, adapter: &str) -> Result<String> {
    match cmd {
        Commands::Start { workflow } => cmd_start(cwd, &workflow),
        Commands::Next => cmd_next(cwd),
        Commands::Report => cmd_report(cwd),
        Commands::Complete { step_id } => cmd_complete(cwd, &step_id),
        Commands::Resume => cmd_resume(cwd),
        Commands::Status => cmd_status(cwd),
        Commands::Validate => cmd_validate(cwd),
        Commands::List => cmd_list(cwd),
        Commands::Hook { event_type } => cmd_hook(cwd, &event_type, adapter),
        Commands::ExecStep { step_id } => cmd_exec_step(cwd, &step_id),
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

fn cmd_next(cwd: &Path) -> Result<String> {
    let config = load_config(cwd)?;
    let state = load_state(cwd)?
        .context("no workflow in progress; run `workflow-runner start <workflow>` first")?;
    let wf = config
        .workflows
        .get(&state.workflow)
        .with_context(|| format!("workflow '{}' not found in config.yml", state.workflow))?;

    let output = executor::build_next(wf, &state, &config);
    Ok(serde_json::to_string_pretty(&output)?)
}

fn cmd_report(cwd: &Path) -> Result<String> {
    let input_str = read_stdin()?;
    let input: ReportInput =
        serde_json::from_str(&input_str).context("report stdin is not valid JSON")?;

    let config = load_config(cwd)?;
    let mut state = load_state(cwd)?.context("no workflow in progress")?;

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

    let is_gate = is_gate_action(wf, &input.step_id, input.action_index);
    {
        let s = state.steps.entry(input.step_id.clone()).or_default();
        if is_gate {
            s.gate_recorded = true;
        }
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

    if is_gate {
        if let Some(stdout) = &input.stdout {
            append_checklist(cwd, stdout)?;
        }
    }

    let out = serde_json::json!({ "ok": true, "step_id": input.step_id });
    Ok(out.to_string())
}

fn cmd_complete(cwd: &Path, step_id: &str) -> Result<String> {
    let config = load_config(cwd)?;
    let mut state = load_state(cwd)?.context("no workflow in progress")?;
    let wf = config
        .workflows
        .get(&state.workflow)
        .with_context(|| format!("workflow '{}' not found", state.workflow))?;

    let gate_result = gate::check(wf, &state, step_id);
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
        clear_state(cwd)?;
    }

    let output = CompleteOutput {
        step_id: step_id.to_string(),
        allowed: true,
        reason: None,
        next: Some(next),
    };
    Ok(serde_json::to_string_pretty(&output)?)
}

fn cmd_resume(cwd: &Path) -> Result<String> {
    let config = load_config(cwd)?;
    let state = load_state(cwd)?.context("no workflow in progress")?;
    let wf = config
        .workflows
        .get(&state.workflow)
        .with_context(|| format!("workflow '{}' not found", state.workflow))?;

    let output = executor::build_next(wf, &state, &config);
    Ok(serde_json::to_string_pretty(&output)?)
}

fn cmd_status(cwd: &Path) -> Result<String> {
    let config = load_config(cwd)?;
    let state = load_state(cwd)?.context("no workflow in progress")?;
    let wf = config
        .workflows
        .get(&state.workflow)
        .with_context(|| format!("workflow '{}' not found", state.workflow))?;

    Ok(serde_json::to_string_pretty(&build_status(&state, wf))?)
}

fn cmd_validate(cwd: &Path) -> Result<String> {
    match load_config(cwd) {
        Ok(config) => {
            let wf_count = config.workflows.len();
            let ok = serde_json::json!({
                "valid": true,
                "workflows": wf_count,
                "commands": config.commands.keys().collect::<Vec<_>>()
            });
            Ok(ok.to_string())
        }
        Err(e) => {
            let err = serde_json::json!({ "valid": false, "error": e.to_string() });
            Ok(err.to_string())
        }
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

    // Hook errors must not crash the process; return empty on failure.
    let result: Option<String> = match event_type {
        "post-bash" => {
            let _ = hook_handler::handle_post_bash(cwd, &input);
            None
        }
        "pre-taskupdate" => hook_handler::handle_pre_taskupdate(cwd, &input).unwrap_or(None),
        "post-edit" => hook_handler::handle_post_edit(cwd, &input).unwrap_or(None),
        _ => None,
    };

    Ok(result.unwrap_or_default())
}

fn read_stdin() -> Result<String> {
    let mut buf = String::new();
    io::stdin().read_to_string(&mut buf)?;
    Ok(buf)
}

fn is_gate_action(wf: &crate::config::types::Workflow, step_id: &str, action_index: usize) -> bool {
    use crate::config::types::Action;

    let (cfg_step_id, sub_id) = if let Some(idx) = step_id.find('/') {
        (&step_id[..idx], Some(&step_id[idx + 1..]))
    } else {
        (step_id, None)
    };

    let step = match wf.steps.iter().find(|s| s.id == cfg_step_id) {
        Some(s) => s,
        None => return false,
    };

    let actions: &[Action] = if let Some(sub) = sub_id {
        let parallel = step.parallel.as_deref().unwrap_or(&[]);
        match parallel.iter().find(|s| s.id == sub) {
            Some(s) => &s.actions,
            None => return false,
        }
    } else {
        &step.actions
    };

    actions
        .get(action_index)
        .map(|a| matches!(a, Action::Run { gate: true, .. }))
        .unwrap_or(false)
}

fn cmd_exec_step(cwd: &Path, step_id: &str) -> Result<String> {
    use crate::config::types::Action;

    let config = load_config(cwd)?;
    let state = load_state(cwd)?.context("no workflow in progress")?;
    let wf = config
        .workflows
        .get(&state.workflow)
        .with_context(|| format!("workflow '{}' not found", state.workflow))?;

    let step = wf
        .steps
        .iter()
        .find(|s| s.id == step_id)
        .with_context(|| format!("step '{}' not found in workflow", step_id))?;

    let session_id = state.session_id.clone();

    for (idx, action) in step.actions.iter().enumerate() {
        let (exit_code, stdout, stderr) = match action {
            Action::Run { command, .. } => {
                let resolved = engine::executor::resolve_template(command, &config);
                let result = standalone_runner::run_command(&resolved, cwd)?;
                eprintln!("{}", result.stderr.trim_end());
                print!("{}", result.stdout);
                (result.exit_code, result.stdout, result.stderr)
            }
            Action::Agent { prompt, .. } => {
                let text = standalone_runner::call_anthropic_api(prompt)?;
                println!("{}", text);
                (0, text, String::new())
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

        let report = serde_json::json!({
            "session_id": session_id,
            "step_id": step_id,
            "action_index": idx,
            "action_type": action_type_name(action),
            "exit_code": exit_code,
            "stdout": stdout,
            "stderr": stderr
        });

        let input: protocol::input::ReportInput = serde_json::from_value(report)?;
        {
            use chrono::Utc;
            use engine::state::{ActionReport, StepStatus};
            let mut st = load_state(cwd)?.context("no workflow in progress")?;
            {
                let s = st.steps.entry(step_id.to_string()).or_default();
                if s.status == StepStatus::Pending {
                    s.status = StepStatus::InProgress;
                    s.started_at = Some(Utc::now());
                }
                let is_gate = is_gate_action(wf, step_id, input.action_index);
                if is_gate {
                    s.gate_recorded = true;
                }
                s.action_reports.push(ActionReport {
                    action_index: input.action_index,
                    action_type: input.action_type.clone(),
                    exit_code: input.exit_code,
                    stdout: input.stdout.clone(),
                    recorded_at: Utc::now(),
                });
                if is_gate {
                    if let Some(out) = &input.stdout {
                        append_checklist(cwd, out)?;
                    }
                }
            }
            save_state(cwd, &st)?;
        }

        if exit_code != 0 {
            anyhow::bail!(
                "command exited with code {}; aborting step '{}'",
                exit_code,
                step_id
            );
        }
    }

    cmd_complete(cwd, step_id)
}

fn action_type_name(action: &crate::config::types::Action) -> &'static str {
    use crate::config::types::Action;
    match action {
        Action::Run { .. } => "run",
        Action::Agent { .. } => "agent",
        Action::Skill { .. } => "skill",
        Action::Workflow { .. } => "workflow",
    }
}

fn append_checklist(cwd: &Path, stdout: &str) -> Result<()> {
    use chrono::Local;
    let path = cwd.join(".workflow/checklist.md");
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let entry = format!("## Test run: {}\n\n```\n{}\n```\n\n", timestamp, stdout);
    let mut existing = std::fs::read_to_string(&path).unwrap_or_default();
    existing.push_str(&entry);
    std::fs::write(path, existing)?;
    Ok(())
}
