use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

/// Execute a prompt via Claude CLI in non-interactive (-p) mode.
/// Does not require ANTHROPIC_API_KEY; uses Claude Code session credentials.
pub fn run_prompt(prompt: &str, cwd: &Path) -> Result<String> {
    let output = Command::new("claude")
        .arg("-p")
        .arg(prompt)
        .current_dir(cwd)
        .output()
        .context("failed to spawn claude; ensure 'claude' is in PATH and authenticated")?;

    let exit_code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    if exit_code != 0 {
        bail!(
            "claude exited with code {}: {}",
            exit_code,
            stderr.trim_end()
        );
    }

    Ok(stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_prompt_fails_when_claude_not_found() {
        // PATH に claude がない環境でも、エラーメッセージが anyhow::Error として返ること
        // (PATH に claude がある場合は -p オプションで実際に呼び出されるため skip)
        let which = std::process::Command::new("which")
            .arg("claude")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if which {
            // claude が存在する環境ではこのテストをスキップ
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let result = run_prompt("hello", dir.path());
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("claude"),
            "error should mention 'claude': {}",
            msg
        );
    }
}
