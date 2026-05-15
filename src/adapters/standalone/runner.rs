use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::Path;
use std::process::Command;

pub struct RunResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Execute a shell command synchronously, capturing stdout/stderr.
pub fn run_command(command: &str, cwd: &Path) -> Result<RunResult> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("failed to spawn command: {}", command))?;

    Ok(RunResult {
        exit_code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

// --- Anthropic API ---

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const DEFAULT_MODEL: &str = "claude-sonnet-4-6";

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: Option<String>,
}

#[derive(Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
}

/// Call the Anthropic Messages API with a single user prompt.
/// Reads ANTHROPIC_API_KEY from the environment.
pub fn call_anthropic_api(prompt: &str) -> Result<String> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .context("ANTHROPIC_API_KEY environment variable is not set")?;

    let body = serde_json::json!({
        "model": DEFAULT_MODEL,
        "max_tokens": 4096,
        "messages": [{"role": "user", "content": prompt}]
    });

    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(ANTHROPIC_API_URL)
        .header("x-api-key", &api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .context("failed to send request to Anthropic API")?;

    let status = resp.status();
    let text = resp
        .text()
        .context("failed to read Anthropic API response")?;

    if !status.is_success() {
        bail!("Anthropic API returned {}: {}", status, text);
    }

    let parsed: MessagesResponse =
        serde_json::from_str(&text).context("failed to parse Anthropic API response")?;

    let content = parsed
        .content
        .into_iter()
        .filter(|b| b.block_type == "text")
        .filter_map(|b| b.text)
        .collect::<Vec<_>>()
        .join("\n");

    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn run_command_captures_stdout() {
        let dir = temp_dir();
        let result = run_command("echo hello", dir.path()).unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "hello");
        assert!(result.stderr.is_empty());
    }

    #[test]
    fn run_command_captures_non_zero_exit() {
        let dir = temp_dir();
        let result = run_command("exit 42", dir.path()).unwrap();
        assert_eq!(result.exit_code, 42);
    }

    #[test]
    fn run_command_captures_stderr() {
        let dir = temp_dir();
        let result = run_command("echo err >&2", dir.path()).unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stderr.trim(), "err");
    }

    #[test]
    fn run_command_uses_cwd() {
        let dir = temp_dir();
        let expected = dir.path().to_str().unwrap().to_string();
        let result = run_command("pwd", dir.path()).unwrap();
        // On macOS /tmp may be symlinked to /private/tmp — compare suffix
        assert!(
            result
                .stdout
                .trim()
                .ends_with(expected.trim_start_matches("/private")),
            "stdout: {}, expected suffix: {}",
            result.stdout.trim(),
            expected
        );
    }
}
