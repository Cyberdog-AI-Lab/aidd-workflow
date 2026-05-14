mod config;
mod engine;
mod adapters;
mod protocol;

use std::io::{self, Read};
use std::path::PathBuf;
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use chrono::Utc;

use config::loader::load_config;
use engine::state::{
    load_state, save_state, clear_state,
    WorkflowState, StepStatus, ActionReport,
};
use engine::{dag, gate, executor};
use protocol::{
    input::ReportInput,
    output::{
        build_status, CompleteOutput, ErrorOutput,
        WorkflowListItem, FlowStatus,
    },
};
use adapters::claude_code::hook_handler;

#[derive(Parser)]
#[command(name = "workflow-runner", about = "Workflow execution engine for AI tools")]
struct Cli {
    /// アダプター名（claude-code | standalone）
    #[arg(long, default_value = "claude-code")]
    adapter: String,

    /// プロジェクトルートディレクトリ（省略時は現在のディレクトリ）
    #[arg(long)]
    cwd: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// ワークフローを開始して最初のアクション群を返す
    Start {
        workflow: String,
    },
    /// 現在の状態から次に実行すべきアクション群を返す
    Next,
    /// アクション実行結果を記録する（stdin: JSON）
    Report,
    /// ステップを完了にする（ゲートチェック付き）
    Complete {
        step_id: String,
    },
    /// 中断ワークフローの再開情報を返す
    Resume,
    /// 現在の実行状態を JSON で返す
    Status,
    /// config.yml を検証する
    Validate,
    /// 利用可能なワークフロー一覧を返す
    List,
    /// Claude Code フックイベントを処理する（stdin: hook JSON）
    Hook {
        /// イベント種別（post-bash | pre-taskupdate | post-edit）
        event_type: String,
    },
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
            let out = serde_json::to_string(&ErrorOutput { error: e.to_string() }).unwrap();
            eprintln!("{}", out);
            std::process::exit(1);
        }
    }
}

fn run(cmd: Commands, cwd: &PathBuf, adapter: &str) -> Result<String> {
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
    }
}

// ─── start ──────────────────────────────────────────────────────────────────

fn cmd_start(cwd: &PathBuf, workflow_name: &str) -> Result<String> {
    let config = load_config(cwd)?;
    let wf = config.workflows.get(workflow_name)
        .with_context(|| format!("ワークフロー '{}' が config.yml に見つかりません", workflow_name))?;

    let state = WorkflowState::new(workflow_name, wf);
    save_state(cwd, &state)?;

    let output = executor::build_next(wf, &state, &config);
    Ok(serde_json::to_string_pretty(&output)?)
}

// ─── next ───────────────────────────────────────────────────────────────────

fn cmd_next(cwd: &PathBuf) -> Result<String> {
    let config = load_config(cwd)?;
    let state = load_state(cwd)?
        .context("実行中のワークフローがありません。先に workflow-runner start <workflow> を実行してください")?;
    let wf = config.workflows.get(&state.workflow)
        .with_context(|| format!("ワークフロー '{}' が config.yml に見つかりません", state.workflow))?;

    let output = executor::build_next(wf, &state, &config);
    Ok(serde_json::to_string_pretty(&output)?)
}

// ─── report ─────────────────────────────────────────────────────────────────

fn cmd_report(cwd: &PathBuf) -> Result<String> {
    let input_str = read_stdin()?;
    let input: ReportInput = serde_json::from_str(&input_str)
        .context("report の stdin が不正な JSON です")?;

    let config = load_config(cwd)?;
    let mut state = load_state(cwd)?
        .context("実行中のワークフローがありません")?;

    let wf = config.workflows.get(&state.workflow)
        .with_context(|| format!("ワークフロー '{}' が見つかりません", state.workflow))?;

    // ステップ状態を InProgress に遷移（まだなら）
    {
        let s = state.steps.entry(input.step_id.clone()).or_default();
        if s.status == StepStatus::Pending {
            s.status = StepStatus::InProgress;
            s.started_at = Some(Utc::now());
        }
    }

    // gate: true の run アクションなら gate_recorded をセット
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

    // 並列サブステップなら親の状態を同期
    if let Some(parent_id) = dag::parent_of(&input.step_id) {
        state.sync_parallel_parent(parent_id, wf);
    }

    save_state(cwd, &state)?;

    // checklist.md に gate アクションの結果を記録
    if is_gate {
        if let Some(stdout) = &input.stdout {
            append_checklist(cwd, stdout)?;
        }
    }

    let out = serde_json::json!({ "ok": true, "step_id": input.step_id });
    Ok(out.to_string())
}

// ─── complete ───────────────────────────────────────────────────────────────

fn cmd_complete(cwd: &PathBuf, step_id: &str) -> Result<String> {
    let config = load_config(cwd)?;
    let mut state = load_state(cwd)?
        .context("実行中のワークフローがありません")?;
    let wf = config.workflows.get(&state.workflow)
        .with_context(|| format!("ワークフロー '{}' が見つかりません", state.workflow))?;

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

    // ステップを Completed に遷移
    {
        let s = state.steps.entry(step_id.to_string()).or_default();
        s.status = StepStatus::Completed;
        s.completed_at = Some(Utc::now());
    }

    // 並列サブステップなら親を同期
    if let Some(parent_id) = dag::parent_of(step_id) {
        state.sync_parallel_parent(parent_id, wf);
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

// ─── resume ─────────────────────────────────────────────────────────────────

fn cmd_resume(cwd: &PathBuf) -> Result<String> {
    let config = load_config(cwd)?;
    let state = load_state(cwd)?
        .context("実行中のワークフローがありません")?;
    let wf = config.workflows.get(&state.workflow)
        .with_context(|| format!("ワークフロー '{}' が見つかりません", state.workflow))?;

    let output = executor::build_next(wf, &state, &config);
    Ok(serde_json::to_string_pretty(&output)?)
}

// ─── status ─────────────────────────────────────────────────────────────────

fn cmd_status(cwd: &PathBuf) -> Result<String> {
    let config = load_config(cwd)?;
    let state = load_state(cwd)?
        .context("実行中のワークフローがありません")?;
    let wf = config.workflows.get(&state.workflow)
        .with_context(|| format!("ワークフロー '{}' が見つかりません", state.workflow))?;

    Ok(serde_json::to_string_pretty(&build_status(&state, wf))?)
}

// ─── validate ───────────────────────────────────────────────────────────────

fn cmd_validate(cwd: &PathBuf) -> Result<String> {
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

// ─── list ───────────────────────────────────────────────────────────────────

fn cmd_list(cwd: &PathBuf) -> Result<String> {
    let config = load_config(cwd)?;
    let mut items: Vec<WorkflowListItem> = config.workflows.iter().map(|(slug, wf)| {
        WorkflowListItem {
            slug: slug.clone(),
            name: wf.name.clone(),
            description: wf.description.clone(),
            step_count: wf.steps.len(),
        }
    }).collect();
    items.sort_by(|a, b| a.slug.cmp(&b.slug));
    Ok(serde_json::to_string_pretty(&items)?)
}

// ─── hook ───────────────────────────────────────────────────────────────────

fn cmd_hook(cwd: &PathBuf, event_type: &str, _adapter: &str) -> Result<String> {
    let input = read_stdin().unwrap_or_default();

    // フックはエラーで落としてはいけないため、エラーは無視して空を返す
    let result: Option<String> = match event_type {
        "post-bash" => {
            let _ = hook_handler::handle_post_bash(cwd, &input);
            None
        }
        "pre-taskupdate" => {
            hook_handler::handle_pre_taskupdate(cwd, &input)
                .unwrap_or(None)
        }
        "post-edit" => {
            hook_handler::handle_post_edit(cwd, &input)
                .unwrap_or(None)
        }
        _ => None,
    };

    Ok(result.unwrap_or_default())
}

// ─── helpers ────────────────────────────────────────────────────────────────

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

    actions.get(action_index)
        .map(|a| matches!(a, Action::Run { gate: true, .. }))
        .unwrap_or(false)
}

fn append_checklist(cwd: &PathBuf, stdout: &str) -> Result<()> {
    use chrono::Local;
    let path = cwd.join(".workflow/checklist.md");
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let entry = format!("## テスト実行: {}\n\n```\n{}\n```\n\n", timestamp, stdout);
    let mut existing = std::fs::read_to_string(&path).unwrap_or_default();
    existing.push_str(&entry);
    std::fs::write(path, existing)?;
    Ok(())
}
