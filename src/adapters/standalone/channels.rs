use anyhow::Result;
use std::path::Path;

use crate::providers::channels;

pub struct AgentResult {
    pub stdout: String,
}

/// Run an agent prompt via Claude Code Channels (claude -p).
pub fn run_agent(prompt: &str, cwd: &Path) -> Result<AgentResult> {
    let stdout = channels::run_prompt(prompt, cwd)?;
    Ok(AgentResult { stdout })
}
