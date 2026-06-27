use super::{
    CommandMode, RuntimeConfig, RuntimeMode, ToolDefinition, ToolPermission, path,
    process::read_limited_child_output,
    result::{ToolError, ToolResult, ToolResultMetadata},
};
use serde_json::{Value, json};
use std::{ffi::OsStr, process::Stdio, time::Instant};
use tokio::process::Command;

pub const EXEC_COMMAND: &str = "exec_command";

pub fn definition() -> ToolDefinition {
    ToolDefinition {
        name: EXEC_COMMAND.to_string(),
        description: "Execute a development command inside the workspace.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "cmd": { "type": "string" },
                "cwd": { "type": "string" },
                "timeout_ms": { "type": "integer", "minimum": 1 }
            },
            "required": ["cmd"],
            "additionalProperties": false
        }),
        permission: ToolPermission::ExecuteCommand,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{CommandMode, RuntimeConfig};
    use serde_json::json;
    use std::path::{Path, PathBuf};

    #[tokio::test]
    async fn exec_command_runs_simple_command_inside_workspace() {
        let root = unique_test_dir("command-simple");
        std::fs::create_dir_all(&root).unwrap();
        let config = allowed_config(&root);

        let result = execute(
            &config,
            "call-1",
            json!({ "cmd": "printf hello", "cwd": "." }),
            Instant::now(),
        )
        .await;

        assert!(result.ok);
        let data = result.data.unwrap();
        assert_eq!(data["cmd"], "printf hello");
        assert_eq!(data["cwd"], ".");
        assert_eq!(data["exit_code"], 0);
        assert_eq!(data["stdout"], "hello");
        assert_eq!(data["stderr"], "");
        assert_eq!(data["timed_out"], false);
        assert_eq!(data["terminated_by_runtime"], false);
        remove_test_dir(root);
    }

    #[tokio::test]
    async fn exec_command_reports_non_zero_exit_code() {
        let root = unique_test_dir("command-non-zero");
        std::fs::create_dir_all(&root).unwrap();
        let config = allowed_config(&root);

        let result = execute(
            &config,
            "call-1",
            json!({ "cmd": "printf nope >&2; exit 7" }),
            Instant::now(),
        )
        .await;

        assert!(result.ok);
        let data = result.data.unwrap();
        assert_eq!(data["exit_code"], 7);
        assert_eq!(data["stdout"], "");
        assert_eq!(data["stderr"], "nope");
        remove_test_dir(root);
    }

    #[tokio::test]
    async fn exec_command_rejects_command_when_disabled() {
        let root = unique_test_dir("command-disabled");
        std::fs::create_dir_all(&root).unwrap();
        let config = RuntimeConfig::workspace_write(&root, &root);

        let result = execute(
            &config,
            "call-1",
            json!({ "cmd": "printf hello" }),
            Instant::now(),
        )
        .await;

        assert!(!result.ok);
        assert_eq!(result.error.unwrap().code, "command_disabled");
        remove_test_dir(root);
    }

    #[tokio::test]
    async fn exec_command_rejects_read_only_even_when_allowed() {
        let root = unique_test_dir("command-read-only");
        std::fs::create_dir_all(&root).unwrap();
        let config = RuntimeConfig::read_only(&root, &root).with_command_mode(CommandMode::Allowed);

        let result = execute(
            &config,
            "call-1",
            json!({ "cmd": "printf hello" }),
            Instant::now(),
        )
        .await;

        assert!(!result.ok);
        assert_eq!(result.error.unwrap().code, "permission_denied");
        remove_test_dir(root);
    }

    #[tokio::test]
    async fn exec_command_rejects_workspace_escape_cwd() {
        let root = unique_test_dir("command-cwd-root");
        let outside = unique_test_dir("command-cwd-outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let config = allowed_config(&root);

        let result = execute(
            &config,
            "call-1",
            json!({ "cmd": "printf hello", "cwd": outside }),
            Instant::now(),
        )
        .await;

        assert!(!result.ok);
        assert_eq!(result.error.unwrap().code, "path_outside_workspace");
        remove_test_dir(root);
        remove_test_dir(outside);
    }

    #[tokio::test]
    async fn exec_command_blocks_denylisted_command_forms() {
        let root = unique_test_dir("command-denylist");
        std::fs::create_dir_all(&root).unwrap();
        let config = allowed_config(&root);

        for cmd in [
            "git reset --hard",
            "git clean -fd",
            "rm -rf /",
            "rm -rf .",
            "sudo true",
            "shutdown now",
            "reboot",
            "mkfs.ext4 /dev/disk",
        ] {
            let result = execute(&config, "call-1", json!({ "cmd": cmd }), Instant::now()).await;

            assert!(!result.ok, "{cmd} should be denied");
            assert_eq!(result.error.unwrap().code, "command_denied");
        }
        remove_test_dir(root);
    }

    #[tokio::test]
    async fn exec_command_times_out_and_stops_child() {
        let root = unique_test_dir("command-timeout");
        std::fs::create_dir_all(&root).unwrap();
        let mut config = allowed_config(&root);
        config.tool_timeout_ms = 50;

        let result = execute(
            &config,
            "call-1",
            json!({
                "cmd": "sleep 1; printf late > late.txt",
                "timeout_ms": 25
            }),
            Instant::now(),
        )
        .await;

        assert!(!result.ok);
        let error = result.error.unwrap();
        assert_eq!(error.code, "timeout");
        assert!(error.retryable);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(!root.join("late.txt").exists());
        remove_test_dir(root);
    }

    #[tokio::test]
    async fn exec_command_truncates_large_stdout() {
        let root = unique_test_dir("command-truncate");
        std::fs::create_dir_all(&root).unwrap();
        let mut config = allowed_config(&root);
        config.output_limit_bytes = 4;

        let result = execute(
            &config,
            "call-1",
            json!({ "cmd": "printf abcdef" }),
            Instant::now(),
        )
        .await;

        assert!(result.ok);
        assert!(result.metadata.stdout_truncated);
        assert!(result.metadata.output_truncated);
        let data = result.data.unwrap();
        assert_eq!(data["stdout"], "abcd");
        assert_eq!(data["exit_code"], Value::Null);
        assert_eq!(data["terminated_by_runtime"], true);
        remove_test_dir(root);
    }

    fn allowed_config(root: &Path) -> RuntimeConfig {
        RuntimeConfig::workspace_write(root, root).with_command_mode(CommandMode::Allowed)
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("generalagent-{name}-{}", uuid::Uuid::new_v4()))
    }

    fn remove_test_dir(path: PathBuf) {
        if path.exists() {
            std::fs::remove_dir_all(path).unwrap();
        }
    }
}

pub async fn execute(
    config: &RuntimeConfig,
    call_id: &str,
    arguments: Value,
    started: Instant,
) -> ToolResult {
    if config.command_mode == CommandMode::Disabled {
        return failure(
            call_id,
            "command_disabled",
            "command execution is disabled",
            false,
            started,
        );
    }
    if config.mode != RuntimeMode::WorkspaceWrite {
        return failure(
            call_id,
            "permission_denied",
            "command execution requires workspace-write runtime mode",
            false,
            started,
        );
    }

    let request = match CommandRequest::parse(&arguments, config.tool_timeout_ms) {
        Ok(request) => request,
        Err(message) => return failure(call_id, "invalid_arguments", message, false, started),
    };
    if command_denied(&request.cmd) {
        return failure(
            call_id,
            "command_denied",
            "command is denied by runtime policy",
            false,
            started,
        );
    }

    let cwd = match path::resolve_existing_workspace_path(&config.workspace_root, &request.cwd) {
        Ok(cwd) => cwd,
        Err(error) => return path_failure(call_id, error.to_string(), started),
    };

    match run_command(config, call_id, request, cwd, started).await {
        Ok(result) => result,
        Err(error) => failure(
            call_id,
            "execution_failed",
            error.to_string(),
            false,
            started,
        ),
    }
}

#[derive(Debug)]
struct CommandRequest {
    cmd: String,
    cwd: String,
    timeout_ms: u64,
}

impl CommandRequest {
    fn parse(arguments: &Value, max_timeout_ms: u64) -> Result<Self, String> {
        let object = arguments
            .as_object()
            .ok_or_else(|| "arguments must be an object".to_string())?;
        let cmd = object
            .get("cmd")
            .and_then(Value::as_str)
            .ok_or_else(|| "cmd must be a string".to_string())?
            .to_string();
        if cmd.trim().is_empty() {
            return Err("cmd must not be empty".to_string());
        }

        let cwd = match object.get("cwd") {
            Some(value) => {
                let cwd = value
                    .as_str()
                    .ok_or_else(|| "cwd must be a string".to_string())?;
                if cwd.is_empty() {
                    return Err("cwd must not be empty".to_string());
                }
                cwd.to_string()
            }
            None => ".".to_string(),
        };

        let timeout_ms = match object.get("timeout_ms") {
            Some(value) => value.as_u64().ok_or_else(|| {
                "timeout_ms must be an integer from 1 to the configured maximum".to_string()
            })?,
            None => max_timeout_ms,
        };
        if timeout_ms == 0 || timeout_ms > max_timeout_ms {
            return Err("timeout_ms must be from 1 to the configured maximum".to_string());
        }

        Ok(Self {
            cmd,
            cwd,
            timeout_ms,
        })
    }
}

async fn run_command(
    config: &RuntimeConfig,
    call_id: &str,
    request: CommandRequest,
    cwd: path::WorkspacePath,
    started: Instant,
) -> anyhow::Result<ToolResult> {
    let mut command = shell_command(&request.cmd);
    command
        .current_dir(&cwd.absolute)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env_clear();
    apply_allowed_env(&mut command);

    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("command stdout unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("command stderr unavailable"))?;
    let output = read_limited_child_output(stdout, stderr, config.output_limit_bytes);
    tokio::pin!(output);

    tokio::select! {
        output = &mut output => {
            let output = output?;
            if output.stdout_truncated || output.stderr_truncated {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Ok(success(
                    call_id,
                    &request.cmd,
                    &cwd.relative,
                    None,
                    output,
                    true,
                    started,
                ));
            }

            let status = child.wait().await?;
            Ok(success(
                call_id,
                &request.cmd,
                &cwd.relative,
                status.code(),
                output,
                false,
                started,
            ))
        }
        _ = tokio::time::sleep(std::time::Duration::from_millis(request.timeout_ms)) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            Ok(failure(
                call_id,
                "timeout",
                "command execution timed out",
                true,
                started,
            ))
        }
    }
}

#[cfg(unix)]
fn shell_command(cmd: &str) -> Command {
    let mut command = Command::new("sh");
    command.arg("-c").arg(cmd);
    command
}

#[cfg(windows)]
fn shell_command(cmd: &str) -> Command {
    let mut command = Command::new("cmd");
    command.arg("/C").arg(cmd);
    command
}

fn apply_allowed_env(command: &mut Command) {
    for key in [
        "PATH", "HOME", "TMPDIR", "TEMP", "TMP", "USER", "SHELL", "LANG", "LC_ALL",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
}

#[derive(Clone, Copy)]
struct DenyRule {
    name: &'static str,
    denied: fn(&str) -> bool,
}

const DENY_RULES: &[DenyRule] = &[
    DenyRule {
        name: "git reset --hard",
        denied: |cmd| contains_words(cmd, &["git", "reset", "--hard"]),
    },
    DenyRule {
        name: "git clean -fd",
        denied: |cmd| contains_words(cmd, &["git", "clean", "-fd"]),
    },
    DenyRule {
        name: "rm -rf /",
        denied: |cmd| contains_words(cmd, &["rm", "-rf", "/"]),
    },
    DenyRule {
        name: "rm -rf .",
        denied: |cmd| contains_words(cmd, &["rm", "-rf", "."]),
    },
    DenyRule {
        name: "sudo",
        denied: |cmd| starts_with_word(cmd, "sudo"),
    },
    DenyRule {
        name: "shutdown",
        denied: |cmd| starts_with_word(cmd, "shutdown"),
    },
    DenyRule {
        name: "reboot",
        denied: |cmd| starts_with_word(cmd, "reboot"),
    },
    DenyRule {
        name: "mkfs",
        denied: |cmd| first_word_starts_with(cmd, "mkfs"),
    },
];

fn command_denied(cmd: &str) -> bool {
    DENY_RULES.iter().any(|rule| {
        let _ = rule.name;
        (rule.denied)(cmd)
    })
}

fn contains_words(cmd: &str, words: &[&str]) -> bool {
    let tokens = command_tokens(cmd);
    tokens.windows(words.len()).any(|window| window == words)
}

fn starts_with_word(cmd: &str, word: &str) -> bool {
    command_tokens(cmd)
        .first()
        .is_some_and(|token| token == word)
}

fn first_word_starts_with(cmd: &str, prefix: &str) -> bool {
    command_tokens(cmd)
        .first()
        .is_some_and(|token| token.starts_with(prefix))
}

fn command_tokens(cmd: &str) -> Vec<String> {
    cmd.split(|character: char| character.is_whitespace() || matches!(character, ';' | '&' | '|'))
        .filter(|token| !token.is_empty())
        .map(|token| token.to_ascii_lowercase())
        .collect()
}

fn success(
    call_id: &str,
    cmd: &str,
    cwd: &std::path::Path,
    exit_code: Option<i32>,
    output: super::process::LimitedChildOutput,
    terminated_by_runtime: bool,
    started: Instant,
) -> ToolResult {
    let stdout_truncated = output.stdout_truncated;
    let stderr_truncated = output.stderr_truncated;
    ToolResult::success(
        EXEC_COMMAND,
        call_id,
        json!({
            "cmd": cmd,
            "cwd": relative_path(cwd),
            "exit_code": exit_code,
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
            "timed_out": false,
            "terminated_by_runtime": terminated_by_runtime
        }),
        ToolResultMetadata {
            duration_ms: started.elapsed().as_millis() as u64,
            stdout_truncated,
            stderr_truncated,
            output_truncated: stdout_truncated || stderr_truncated,
        },
    )
}

fn failure(
    call_id: &str,
    code: &str,
    message: impl Into<String>,
    retryable: bool,
    started: Instant,
) -> ToolResult {
    ToolResult::failure(
        EXEC_COMMAND,
        call_id,
        ToolError {
            code: code.to_string(),
            message: message.into(),
            retryable,
        },
        ToolResultMetadata {
            duration_ms: started.elapsed().as_millis() as u64,
            ..ToolResultMetadata::default()
        },
    )
}

fn path_failure(call_id: &str, message: String, started: Instant) -> ToolResult {
    let code = if message.contains("outside workspace") || message.contains("parent traversal") {
        "path_outside_workspace"
    } else if message.contains("failed to resolve workspace path") {
        "path_not_found"
    } else {
        "invalid_arguments"
    };
    failure(call_id, code, message, false, started)
}

fn relative_path(path: &std::path::Path) -> String {
    if path.as_os_str() == OsStr::new("") {
        ".".to_string()
    } else {
        path.to_string_lossy().to_string()
    }
}
