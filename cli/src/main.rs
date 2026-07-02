mod adapters;
mod cmd;
mod config;
mod engine;
mod infra;
mod protocol;
mod providers;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use adapters::hooks::hook_handler;
use config::loader::{load_and_merge_config, load_config, validate as validate_config};
use engine::state::WorkflowState;
use engine::store::{find_workflow_id_by_status, load_state, load_state_by_id};
use protocol::output::{
    build_status, format_status_table, format_validate_text, ErrorOutput, ValidateOutput,
    WorkflowListItem,
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
    /// Start the callback/orchestrator daemon (binds the HTTP server, awaits callbacks).
    /// Starts with zero workflows running; use `run` to start one on it.
    Serve {
        /// Port for the local callback HTTP server (conflicts with --callback-url) [default: 8789].
        #[arg(long, conflicts_with = "callback_url")]
        callback_port: Option<u16>,
        /// Full callback URL sent to Claude Code, e.g. an ngrok tunnel (conflicts with --callback-port).
        /// The local server always binds on the port from --callback-port (default 8789).
        #[arg(long, conflicts_with = "callback_port")]
        callback_url: Option<String>,
        /// Port of the Channels webhook server (conflicts with --webhook-url) [default: 8788].
        #[arg(long, conflicts_with = "webhook_url")]
        webhook_port: Option<u16>,
        /// Full URL of the Channels webhook server (conflicts with --webhook-port).
        #[arg(long, conflicts_with = "webhook_port")]
        webhook_url: Option<String>,
    },
    /// Start a workflow on a running `workflow-runner serve` daemon
    /// (POST /run on its callback server). Prints the newly assigned workflow_id.
    /// Call it multiple times to run several workflows concurrently on one daemon.
    Run {
        workflow: String,
        /// Port of the running daemon's callback server (conflicts with --callback-url) [default: 8789].
        #[arg(long, conflicts_with = "callback_url")]
        callback_port: Option<u16>,
        /// Full callback base URL of the running daemon (conflicts with --callback-port).
        #[arg(long, conflicts_with = "callback_port")]
        callback_url: Option<String>,
    },
    /// Stop a running `workflow-runner serve` daemon (POST /stop on its callback server).
    Stop {
        /// Port of the running daemon's callback server (conflicts with --callback-url) [default: 8789].
        #[arg(long, conflicts_with = "callback_url")]
        callback_port: Option<u16>,
        /// Full callback base URL of the running daemon (conflicts with --callback-port).
        #[arg(long, conflicts_with = "callback_port")]
        callback_url: Option<String>,
    },
    /// Approve an awaiting-approval workflow on a running `workflow-runner serve` daemon
    /// (POST /approve/:workflow_id on its callback server). Uses the global --workflow-id
    /// if given; otherwise auto-selects the single awaiting-approval workflow, erroring
    /// if there is more than one.
    Approve {
        /// Port of the running daemon's callback server (conflicts with --callback-url) [default: 8789].
        #[arg(long, conflicts_with = "callback_url")]
        callback_port: Option<u16>,
        /// Full callback base URL of the running daemon (conflicts with --callback-port).
        #[arg(long, conflicts_with = "callback_port")]
        callback_url: Option<String>,
    },
    /// Resume a paused workflow (e.g. after an agent called `pause`) on a running
    /// `workflow-runner serve` daemon (POST /resume/:workflow_id on its callback server).
    /// Uses the global --workflow-id if given; otherwise auto-selects the single paused
    /// workflow, erroring if there is more than one.
    Resume {
        /// Port of the running daemon's callback server (conflicts with --callback-url) [default: 8789].
        #[arg(long, conflicts_with = "callback_url")]
        callback_port: Option<u16>,
        /// Full callback base URL of the running daemon (conflicts with --callback-port).
        #[arg(long, conflicts_with = "callback_port")]
        callback_url: Option<String>,
    },
    /// Reject an awaiting-approval task and retry it, on a running
    /// `workflow-runner serve` daemon (POST /reject/:workflow_id/:task_id on its
    /// callback server). Uses the global --workflow-id if given; otherwise auto-selects
    /// the single awaiting-approval workflow, erroring if there is more than one.
    Reject {
        task_id: String,
        /// Developer feedback explaining the rejection.
        #[arg(long)]
        reason: Option<String>,
        /// Port of the running daemon's callback server (conflicts with --callback-url) [default: 8789].
        #[arg(long, conflicts_with = "callback_url")]
        callback_port: Option<u16>,
        /// Full callback base URL of the running daemon (conflicts with --callback-port).
        #[arg(long, conflicts_with = "callback_port")]
        callback_url: Option<String>,
    },
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
        Commands::Serve {
            callback_port,
            callback_url,
            webhook_port,
            webhook_url,
        } => cmd_serve(cwd, callback_port, callback_url, webhook_port, webhook_url),
        Commands::Run {
            workflow,
            callback_port,
            callback_url,
        } => cmd_run(&workflow, callback_port, callback_url),
        Commands::Stop {
            callback_port,
            callback_url,
        } => cmd_stop(callback_port, callback_url),
        Commands::Approve {
            callback_port,
            callback_url,
        } => cmd_approve(cwd, workflow_id, callback_port, callback_url),
        Commands::Resume {
            callback_port,
            callback_url,
        } => cmd_resume(cwd, workflow_id, callback_port, callback_url),
        Commands::Reject {
            task_id,
            reason,
            callback_port,
            callback_url,
        } => cmd_reject(
            cwd,
            workflow_id,
            &task_id,
            reason.as_deref(),
            callback_port,
            callback_url,
        ),
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

/// Resolves the daemon's callback base URL from CLI flags, matching the
/// defaulting logic used by `workflow-runner serve` itself.
fn resolve_callback_base_url(callback_port: Option<u16>, callback_url: Option<String>) -> String {
    callback_url.unwrap_or_else(|| format!("http://127.0.0.1:{}", callback_port.unwrap_or(8789)))
}

/// Resolves the workflow_id to target for approve/resume/reject.
/// Uses the explicit `--workflow-id` if given; otherwise auto-selects the
/// single workflow in the local store matching `status`, erroring with the
/// list of candidates (via `find_workflow_id_by_status`) if more than one
/// qualifies, or a clear message if none do.
fn resolve_target_workflow_id(
    cwd: &Path,
    workflow_id: Option<&str>,
    status: &str,
) -> Result<String> {
    if let Some(id) = workflow_id {
        return Ok(id.to_string());
    }
    find_workflow_id_by_status(cwd, status)?.with_context(|| {
        format!(
            "no workflow with status '{}' found; specify --workflow-id explicitly",
            status
        )
    })
}

/// POSTs to a running `workflow-runner serve` daemon's callback server.
/// The daemon replies unconditionally with `"ok"` regardless of whether the
/// event applied (e.g. approving when nothing is awaiting approval), so this
/// only confirms that the daemon was reachable, not that the action took effect.
fn post_to_daemon(url: &str, body: Option<serde_json::Value>) -> Result<()> {
    let client = reqwest::blocking::Client::new();
    let req = match body {
        Some(b) => client.post(url).json(&b),
        None => client.post(url),
    };
    req.send().with_context(|| {
        format!(
            "failed to reach workflow-runner serve daemon at {}; is `workflow-runner serve` running?",
            url
        )
    })?;
    Ok(())
}

/// POSTs JSON to a running daemon and returns the raw response body, treating
/// a non-2xx status as an error. Unlike `post_to_daemon`, this is used where
/// the caller needs to observe the daemon's synchronous result (e.g. `/run`,
/// which can fail if the workflow name is not defined in config.yml).
fn post_to_daemon_expect_ok(url: &str, body: serde_json::Value) -> Result<String> {
    let client = reqwest::blocking::Client::new();
    let resp = client.post(url).json(&body).send().with_context(|| {
        format!(
            "failed to reach workflow-runner serve daemon at {}; is `workflow-runner serve` running?",
            url
        )
    })?;
    let status = resp.status();
    let text = resp
        .text()
        .context("failed to read workflow-runner serve daemon response body")?;
    if !status.is_success() {
        anyhow::bail!("daemon returned an error: {}", text);
    }
    Ok(text)
}

fn cmd_serve(
    cwd: &Path,
    callback_port: Option<u16>,
    callback_url: Option<String>,
    webhook_port: Option<u16>,
    webhook_url: Option<String>,
) -> Result<String> {
    let cb_port = callback_port.unwrap_or(8789);
    let cb_url = callback_url.unwrap_or_else(|| format!("http://127.0.0.1:{}", cb_port));
    let wh_port = webhook_port.unwrap_or(8788);
    let wh_url = webhook_url.unwrap_or_else(|| format!("http://127.0.0.1:{}", wh_port));
    let rt = tokio::runtime::Runtime::new().context("failed to create tokio runtime")?;
    rt.block_on(cmd::run::run_daemon(
        cwd.to_path_buf(),
        cb_port,
        cb_url,
        wh_url,
    ))?;
    Ok(String::new())
}

fn cmd_run(
    workflow: &str,
    callback_port: Option<u16>,
    callback_url: Option<String>,
) -> Result<String> {
    let base = resolve_callback_base_url(callback_port, callback_url);
    let body = serde_json::json!({ "workflow": workflow });
    post_to_daemon_expect_ok(&format!("{}/run", base), body)
}

fn cmd_stop(callback_port: Option<u16>, callback_url: Option<String>) -> Result<String> {
    let base = resolve_callback_base_url(callback_port, callback_url);
    post_to_daemon(&format!("{}/stop", base), None)?;
    Ok(serde_json::json!({ "ok": true, "action": "stop" }).to_string())
}

fn cmd_approve(
    cwd: &Path,
    workflow_id: Option<&str>,
    callback_port: Option<u16>,
    callback_url: Option<String>,
) -> Result<String> {
    let id = resolve_target_workflow_id(cwd, workflow_id, "awaiting_approval")?;
    let base = resolve_callback_base_url(callback_port, callback_url);
    post_to_daemon(&format!("{}/approve/{}", base, id), None)?;
    Ok(serde_json::json!({ "ok": true, "action": "approve", "workflow_id": id }).to_string())
}

fn cmd_resume(
    cwd: &Path,
    workflow_id: Option<&str>,
    callback_port: Option<u16>,
    callback_url: Option<String>,
) -> Result<String> {
    let id = resolve_target_workflow_id(cwd, workflow_id, "paused")?;
    let base = resolve_callback_base_url(callback_port, callback_url);
    post_to_daemon(&format!("{}/resume/{}", base, id), None)?;
    Ok(serde_json::json!({ "ok": true, "action": "resume", "workflow_id": id }).to_string())
}

fn cmd_reject(
    cwd: &Path,
    workflow_id: Option<&str>,
    task_id: &str,
    reason: Option<&str>,
    callback_port: Option<u16>,
    callback_url: Option<String>,
) -> Result<String> {
    let id = resolve_target_workflow_id(cwd, workflow_id, "awaiting_approval")?;
    let base = resolve_callback_base_url(callback_port, callback_url);
    let body = reason.map(|r| serde_json::json!({ "reason": r }));
    post_to_daemon(&format!("{}/reject/{}/{}", base, id, task_id), body)?;
    Ok(serde_json::json!({
        "ok": true,
        "action": "reject",
        "workflow_id": id,
        "task_id": task_id
    })
    .to_string())
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
