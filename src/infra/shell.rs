use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

pub struct RunResult {
    pub exit_code: i32,
    #[allow(dead_code)]
    pub stdout: String,
    #[allow(dead_code)]
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
