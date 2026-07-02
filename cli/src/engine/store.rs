use super::state::{ActionReport, StepState, StepStatus, WorkflowState};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::path::Path;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS workflow_runs (
    workflow_id   TEXT PRIMARY KEY,
    cwd           TEXT NOT NULL,
    workflow      TEXT NOT NULL,
    status        TEXT NOT NULL DEFAULT 'active',
    started_at    TEXT NOT NULL,
    completed_at  TEXT
);

CREATE INDEX IF NOT EXISTS idx_runs_cwd_status
    ON workflow_runs(cwd, status);

CREATE TABLE IF NOT EXISTS step_states (
    workflow_id    TEXT NOT NULL REFERENCES workflow_runs(workflow_id) ON DELETE CASCADE,
    step_id        TEXT NOT NULL,
    status         TEXT NOT NULL DEFAULT 'pending',
    started_at     TEXT,
    completed_at   TEXT,
    updated_at     TEXT,
    PRIMARY KEY (workflow_id, step_id)
);

CREATE TABLE IF NOT EXISTS action_reports (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    workflow_id    TEXT NOT NULL REFERENCES workflow_runs(workflow_id) ON DELETE CASCADE,
    step_id        TEXT NOT NULL,
    action_index   INTEGER NOT NULL,
    action_type    TEXT NOT NULL,
    exit_code      INTEGER,
    stdout         TEXT,
    recorded_at    TEXT NOT NULL
);
";

/// Canonicalizes a path to an absolute, normalized form.
/// This prevents issues with symlinks, trailing slashes, and different path representations.
fn normalize_cwd(cwd: &Path) -> Result<String> {
    let canonical = cwd
        .canonicalize()
        .context("failed to canonicalize cwd path")?;
    Ok(canonical.to_string_lossy().to_string())
}

fn open_db(cwd: &Path) -> Result<Connection> {
    let dir = cwd.join(".workflow");
    std::fs::create_dir_all(&dir)?;
    let conn = Connection::open(dir.join("workflow.db")).context("failed to open workflow.db")?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    conn.execute_batch(SCHEMA)?;
    // Migration: add updated_at column to existing databases (error is ignored if column already exists).
    let _ = conn.execute_batch("ALTER TABLE step_states ADD COLUMN updated_at TEXT;");
    Ok(conn)
}

fn status_to_str(s: &StepStatus) -> &'static str {
    match s {
        StepStatus::Pending => "pending",
        StepStatus::InProgress => "in_progress",
        StepStatus::Completed => "completed",
        StepStatus::Failed => "failed",
    }
}

fn str_to_status(s: &str) -> StepStatus {
    match s {
        "in_progress" => StepStatus::InProgress,
        "completed" => StepStatus::Completed,
        "failed" => StepStatus::Failed,
        _ => StepStatus::Pending,
    }
}

fn load_state_from_db(conn: &Connection, workflow_id: &str) -> Result<WorkflowState> {
    let (workflow, started_at_str): (String, String) = conn
        .query_row(
            "SELECT workflow, started_at FROM workflow_runs WHERE workflow_id = ?1",
            params![workflow_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .context("workflow_run not found")?;

    let started_at: DateTime<Utc> = DateTime::parse_from_rfc3339(&started_at_str)
        .context("invalid started_at timestamp")?
        .with_timezone(&Utc);

    let mut stmt = conn.prepare(
        "SELECT step_id, status, started_at, completed_at, updated_at
         FROM step_states WHERE workflow_id = ?1",
    )?;
    let step_rows = stmt.query_map(params![workflow_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
        ))
    })?;

    let mut tasks: HashMap<String, StepState> = HashMap::new();
    for row in step_rows {
        let (step_id, status_str, started_str, completed_str, updated_str) = row?;
        let started_at_step = started_str
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc));
        let completed_at_step = completed_str
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc));
        let updated_at_step = updated_str
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc));

        tasks.insert(
            step_id,
            StepState {
                status: str_to_status(&status_str),
                started_at: started_at_step,
                completed_at: completed_at_step,
                updated_at: updated_at_step,
                action_reports: vec![],
            },
        );
    }

    let mut stmt = conn.prepare(
        "SELECT step_id, action_index, action_type, exit_code, stdout, recorded_at
         FROM action_reports WHERE workflow_id = ?1 ORDER BY id",
    )?;
    let report_rows = stmt.query_map(params![workflow_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, usize>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<i32>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, String>(5)?,
        ))
    })?;

    for row in report_rows {
        let (step_id, action_index, action_type, exit_code, stdout, recorded_at_str) = row?;
        let recorded_at = DateTime::parse_from_rfc3339(&recorded_at_str)
            .context("invalid recorded_at timestamp")?
            .with_timezone(&Utc);
        let entry = tasks.entry(step_id).or_default();
        entry.action_reports.push(ActionReport {
            action_index,
            action_type,
            exit_code,
            stdout,
            recorded_at,
        });
    }

    Ok(WorkflowState {
        workflow_id: workflow_id.to_string(),
        workflow,
        started_at,
        tasks,
    })
}

/// Loads the single active (or awaiting-approval) workflow for the given cwd.
/// Returns None if no such workflow exists.
/// Returns an error if multiple qualifying workflows exist (use --workflow-id to disambiguate).
pub fn load_state(cwd: &Path) -> Result<Option<WorkflowState>> {
    let conn = open_db(cwd)?;
    let cwd_str = normalize_cwd(cwd)?;

    let mut stmt = conn.prepare(
        "SELECT workflow_id FROM workflow_runs
         WHERE cwd = ?1 AND status IN ('active', 'awaiting_approval', 'paused')",
    )?;
    let ids: Vec<String> = stmt
        .query_map(params![cwd_str], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;

    match ids.len() {
        0 => Ok(None),
        1 => Ok(Some(load_state_from_db(&conn, &ids[0])?)),
        _ => anyhow::bail!(
            "multiple active workflows found; use --workflow-id to specify one\n  {}",
            ids.join("\n  ")
        ),
    }
}

/// Loads a workflow by its explicit workflow_id (active or awaiting-approval).
pub fn load_state_by_id(cwd: &Path, workflow_id: &str) -> Result<Option<WorkflowState>> {
    let conn = open_db(cwd)?;

    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM workflow_runs
             WHERE workflow_id = ?1 AND status IN ('active', 'awaiting_approval', 'paused')",
            params![workflow_id],
            |row| row.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .unwrap_or(false);

    if !exists {
        return Ok(None);
    }

    Ok(Some(load_state_from_db(&conn, workflow_id)?))
}

/// Saves (upserts) the workflow state to SQLite within a single transaction.
/// Does NOT overwrite `status` on update — use `set_workflow_status` to change it.
/// All writes are atomic: a crash mid-save leaves the previous state intact.
pub fn save_state(cwd: &Path, state: &WorkflowState) -> Result<()> {
    let mut conn = open_db(cwd)?;
    let cwd_str = normalize_cwd(cwd)?;

    let tx = conn.transaction()?;

    tx.execute(
        "INSERT INTO workflow_runs (workflow_id, cwd, workflow, status, started_at)
         VALUES (?1, ?2, ?3, 'active', ?4)
         ON CONFLICT(workflow_id) DO UPDATE SET
             cwd = excluded.cwd,
             workflow = excluded.workflow,
             started_at = excluded.started_at",
        params![
            state.workflow_id,
            cwd_str,
            state.workflow,
            state.started_at.to_rfc3339(),
        ],
    )?;

    for (task_id, task) in &state.tasks {
        tx.execute(
            "INSERT OR REPLACE INTO step_states
             (workflow_id, step_id, status, started_at, completed_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                state.workflow_id,
                task_id,
                status_to_str(&task.status),
                task.started_at.map(|dt| dt.to_rfc3339()),
                task.completed_at.map(|dt| dt.to_rfc3339()),
                task.updated_at.map(|dt| dt.to_rfc3339()),
            ],
        )?;
    }

    tx.execute(
        "DELETE FROM action_reports WHERE workflow_id = ?1",
        params![state.workflow_id],
    )?;

    for (task_id, task) in &state.tasks {
        for report in &task.action_reports {
            tx.execute(
                "INSERT INTO action_reports
                 (workflow_id, step_id, action_index, action_type, exit_code, stdout, recorded_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    state.workflow_id,
                    task_id,
                    report.action_index as i64,
                    report.action_type,
                    report.exit_code,
                    report.stdout,
                    report.recorded_at.to_rfc3339(),
                ],
            )?;
        }
    }

    tx.commit()?;
    Ok(())
}

/// Returns the current status string of a workflow_run row.
pub fn get_workflow_status(cwd: &Path, workflow_id: &str) -> Result<String> {
    let conn = open_db(cwd)?;
    let status: String = conn
        .query_row(
            "SELECT status FROM workflow_runs WHERE workflow_id = ?1",
            params![workflow_id],
            |row| row.get(0),
        )
        .context("workflow not found")?;
    Ok(status)
}

/// Updates the status column of a workflow_run row.
pub fn set_workflow_status(cwd: &Path, workflow_id: &str, status: &str) -> Result<()> {
    let conn = open_db(cwd)?;
    conn.execute(
        "UPDATE workflow_runs SET status = ?1 WHERE workflow_id = ?2",
        params![status, workflow_id],
    )?;
    Ok(())
}

/// Marks a specific workflow as completed.
pub fn clear_state_by_id(cwd: &Path, workflow_id: &str) -> Result<()> {
    let conn = open_db(cwd)?;
    conn.execute(
        "UPDATE workflow_runs SET status = 'completed', completed_at = ?1
         WHERE workflow_id = ?2",
        params![Utc::now().to_rfc3339(), workflow_id],
    )?;
    Ok(())
}

/// Sets a task's status to in_progress and records started_at / updated_at.
/// Called by the run process when dispatching a task via webhook.
/// Uses COALESCE so that re-dispatching does not reset started_at.
pub fn mark_task_dispatched(cwd: &Path, workflow_id: &str, task_id: &str) -> Result<()> {
    let conn = open_db(cwd)?;
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE step_states
         SET status = 'in_progress', started_at = COALESCE(started_at, ?1), updated_at = ?1
         WHERE workflow_id = ?2 AND step_id = ?3",
        params![now, workflow_id, task_id],
    )?;
    Ok(())
}

/// Updates the updated_at timestamp of a single step.
/// Used by report and pause handlers to record activity without changing status.
pub fn update_task_timestamp(cwd: &Path, workflow_id: &str, task_id: &str) -> Result<()> {
    let conn = open_db(cwd)?;
    conn.execute(
        "UPDATE step_states SET updated_at = ?1
         WHERE workflow_id = ?2 AND step_id = ?3",
        params![Utc::now().to_rfc3339(), workflow_id, task_id],
    )?;
    Ok(())
}

/// Resolves the single workflow_id in `cwd` whose status matches `status`.
/// Returns `Ok(None)` if none match, `Ok(Some(id))` if exactly one matches,
/// and an error listing all candidates if more than one matches.
///
/// Used by `approve`/`resume`/`reject` CLI commands to auto-select a target
/// workflow when `--workflow-id` is omitted.
pub fn find_workflow_id_by_status(cwd: &Path, status: &str) -> Result<Option<String>> {
    let conn = open_db(cwd)?;
    let cwd_str = normalize_cwd(cwd)?;

    let mut stmt =
        conn.prepare("SELECT workflow_id FROM workflow_runs WHERE cwd = ?1 AND status = ?2")?;
    let ids: Vec<String> = stmt
        .query_map(params![cwd_str, status], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;

    match ids.len() {
        0 => Ok(None),
        1 => Ok(Some(ids[0].clone())),
        _ => anyhow::bail!(
            "multiple workflows with status '{}' found; use --workflow-id to specify one\n  {}",
            status,
            ids.join("\n  ")
        ),
    }
}

/// Marks the single active workflow as completed.
#[allow(dead_code)]
pub fn clear_state(cwd: &Path) -> Result<()> {
    let conn = open_db(cwd)?;
    let cwd_str = normalize_cwd(cwd)?;
    conn.execute(
        "UPDATE workflow_runs SET status = 'completed', completed_at = ?1
         WHERE cwd = ?2 AND status = 'active'",
        params![Utc::now().to_rfc3339(), cwd_str],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{Task, Workflow};
    use tempfile::TempDir;

    fn minimal_workflow() -> Workflow {
        Workflow {
            name: "test".to_string(),
            description: None,
            tasks: vec![Task {
                id: "task1".to_string(),
                task: Some("Task 1".to_string()),
                ..Task::default()
            }],
        }
    }

    #[test]
    fn roundtrip_state() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path();

        let wf = minimal_workflow();
        let state = WorkflowState::new("test", &wf);
        let workflow_id = state.workflow_id.clone();

        save_state(cwd, &state).unwrap();
        let loaded = load_state(cwd).unwrap().unwrap();
        assert_eq!(loaded.workflow_id, workflow_id);
        assert_eq!(loaded.workflow, "test");
    }

    #[test]
    fn load_state_returns_none_when_missing() {
        let dir = TempDir::new().unwrap();
        let result = load_state(dir.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn clear_state_by_id_marks_completed() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path();

        let wf = minimal_workflow();
        let state = WorkflowState::new("test", &wf);
        let id = state.workflow_id.clone();
        save_state(cwd, &state).unwrap();

        assert!(load_state(cwd).unwrap().is_some());
        clear_state_by_id(cwd, &id).unwrap();
        assert!(load_state(cwd).unwrap().is_none());
    }

    #[test]
    fn load_state_by_id_returns_state() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path();

        let wf = minimal_workflow();
        let state = WorkflowState::new("test", &wf);
        let id = state.workflow_id.clone();
        save_state(cwd, &state).unwrap();

        let loaded = load_state_by_id(cwd, &id).unwrap().unwrap();
        assert_eq!(loaded.workflow_id, id);
    }

    #[test]
    fn multiple_active_workflows_error() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path();

        let wf = minimal_workflow();
        let state1 = WorkflowState::new("test", &wf);
        let state2 = WorkflowState::new("test", &wf);
        save_state(cwd, &state1).unwrap();
        save_state(cwd, &state2).unwrap();

        assert!(load_state(cwd).is_err());
    }

    #[test]
    fn action_reports_roundtrip() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path();

        let wf = minimal_workflow();
        let mut state = WorkflowState::new("test", &wf);
        let task = state.tasks.entry("task1".to_string()).or_default();
        task.status = StepStatus::InProgress;
        task.action_reports.push(ActionReport {
            action_index: 0,
            action_type: "agent".to_string(),
            exit_code: Some(0),
            stdout: Some("ok".to_string()),
            recorded_at: Utc::now(),
        });

        save_state(cwd, &state).unwrap();
        let loaded = load_state(cwd).unwrap().unwrap();
        let reports = &loaded.tasks["task1"].action_reports;
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].action_type, "agent");
        assert_eq!(reports[0].stdout.as_deref(), Some("ok"));
    }

    #[test]
    fn awaiting_approval_state_is_loadable() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path();

        let wf = minimal_workflow();
        let state = WorkflowState::new("test", &wf);
        let id = state.workflow_id.clone();
        save_state(cwd, &state).unwrap();

        set_workflow_status(cwd, &id, "awaiting_approval").unwrap();

        // load_state should still find it.
        let loaded = load_state(cwd).unwrap();
        assert!(loaded.is_some());
        assert_eq!(get_workflow_status(cwd, &id).unwrap(), "awaiting_approval");
    }

    #[test]
    fn set_workflow_status_updates_correctly() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path();

        let wf = minimal_workflow();
        let state = WorkflowState::new("test", &wf);
        let id = state.workflow_id.clone();
        save_state(cwd, &state).unwrap();

        assert_eq!(get_workflow_status(cwd, &id).unwrap(), "active");
        set_workflow_status(cwd, &id, "awaiting_approval").unwrap();
        assert_eq!(get_workflow_status(cwd, &id).unwrap(), "awaiting_approval");
        set_workflow_status(cwd, &id, "active").unwrap();
        assert_eq!(get_workflow_status(cwd, &id).unwrap(), "active");
    }

    #[test]
    fn find_workflow_id_by_status_returns_none_when_no_match() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path();

        let wf = minimal_workflow();
        let state = WorkflowState::new("test", &wf);
        save_state(cwd, &state).unwrap();

        let result = find_workflow_id_by_status(cwd, "paused").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn find_workflow_id_by_status_returns_single_match() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path();

        let wf = minimal_workflow();
        let state = WorkflowState::new("test", &wf);
        let id = state.workflow_id.clone();
        save_state(cwd, &state).unwrap();
        set_workflow_status(cwd, &id, "paused").unwrap();

        let result = find_workflow_id_by_status(cwd, "paused").unwrap();
        assert_eq!(result, Some(id));
    }

    #[test]
    fn find_workflow_id_by_status_errors_on_multiple_matches() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path();

        let wf = minimal_workflow();
        let state1 = WorkflowState::new("test", &wf);
        let state2 = WorkflowState::new("test", &wf);
        let id1 = state1.workflow_id.clone();
        let id2 = state2.workflow_id.clone();
        save_state(cwd, &state1).unwrap();
        save_state(cwd, &state2).unwrap();
        set_workflow_status(cwd, &id1, "paused").unwrap();
        set_workflow_status(cwd, &id2, "paused").unwrap();

        let err = find_workflow_id_by_status(cwd, "paused").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains(&id1));
        assert!(msg.contains(&id2));
    }

    #[test]
    fn find_workflow_id_by_status_ignores_other_statuses() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path();

        let wf = minimal_workflow();
        let state1 = WorkflowState::new("test", &wf); // stays 'active'
        let state2 = WorkflowState::new("test", &wf);
        let id2 = state2.workflow_id.clone();
        save_state(cwd, &state1).unwrap();
        save_state(cwd, &state2).unwrap();
        set_workflow_status(cwd, &id2, "paused").unwrap();

        let result = find_workflow_id_by_status(cwd, "paused").unwrap();
        assert_eq!(result, Some(id2));
    }

    #[test]
    fn save_state_does_not_reset_awaiting_approval_status() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path();

        let wf = minimal_workflow();
        let state = WorkflowState::new("test", &wf);
        let id = state.workflow_id.clone();
        save_state(cwd, &state).unwrap();

        set_workflow_status(cwd, &id, "awaiting_approval").unwrap();
        // Calling save_state again must NOT reset status back to 'active'.
        save_state(cwd, &state).unwrap();
        assert_eq!(get_workflow_status(cwd, &id).unwrap(), "awaiting_approval");
    }
}
