# Codex-Like Runtime Phase 2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build Phase 2 of the Codex-like runtime: workspace search, development-only command execution, minimal Codex-style patch application, command safety policy, and verification evidence.

**Architecture:** Keep Phase 1 filesystem tools stable and add Phase 2 tools as separate modules under `crates/agent-runtime/src/tools/`. `BuiltInTools` remains the registry facade, while `search`, `command`, and `patch` own their own parsing, safety checks, execution, and tests. `RuntimeConfig` gains a development-only command mode so packaged/default runs keep command execution unavailable.

**Tech Stack:** Rust 2024, Tokio async filesystem/process APIs, serde/serde_json, existing `ToolResult` envelope, pixi-managed cargo commands.

---

## Scope

Phase 2 scope from the migration design:

- Add `search_files`.
- Add workspace-scoped `exec_command`.
- Add development-only `command_allowed` mode.
- Add table-driven command deny rules.
- Add non-interactive command execution with timeout.
- Add stdout/stderr output truncation metadata.
- Add minimal `apply_patch` support.
- Add tests for search results, command success, command failure, timeout, disabled command mode, deny rules, and outside-workspace patch rejection.

Out of scope:

- Persistent interactive command sessions.
- Approval prompts.
- Sandboxed network policy.
- MCP, connectors, subagents inside GeneralAgent.
- Full Codex patch grammar edge cases such as file moves, binary patches, or multi-file atomic rollback.
- Desktop UI changes.

## Current Context

- Work on `main`, per repository instructions. Do not create a worktree.
- The worktree has unrelated dirty changes in desktop files, `docs/mvp-verification.md`, `pixi.toml`, `crates/agent-runtime/src/skill.rs`, and untracked `.codex/`, `scripts/`, and `apps/desktop/src/renderer/modelSettings.ts`. Do not stage or revert those unless the task explicitly needs them.
- Current line counts before Phase 2:
  - `crates/agent-runtime/src/tools/builtin.rs`: 768 lines.
  - `crates/agent-runtime/src/tools/mod.rs`: 536 lines.
  - `crates/agent-runtime/src/skill.rs`: 745 lines.
  - `crates/agent-server/src/api.rs`: 943 lines.
- Source files must remain under 1000 physical lines. Add new modules instead of growing `builtin.rs` heavily.

## File Structure

Create:

- `crates/agent-runtime/src/tools/search.rs`
  - Owns `search_files` schema, argument parsing, workspace path validation, ripgrep execution, safe fallback search, result limiting, and tests.
- `crates/agent-runtime/src/tools/command.rs`
  - Owns `exec_command` schema, `CommandMode::Allowed` behavior, working-directory validation, deny rules, controlled environment, shell invocation, timeout, output limiting, and tests.
- `crates/agent-runtime/src/tools/process.rs`
  - Owns reusable bounded stdout/stderr capture for child processes. Phase 2 uses it for `exec_command`; a later cleanup can move runtime skill execution onto the same helper without touching Phase 2 behavior.
- `crates/agent-runtime/src/tools/patch.rs`
  - Owns `apply_patch` schema, minimal patch parser/executor, workspace path validation, changed-file summaries, and tests.

Modify:

- `crates/agent-runtime/src/tools/mod.rs`
  - Export new modules.
  - Extend `CommandMode`.
  - Extend `ToolPermission`.
  - Update permission checks and tests.
- `crates/agent-runtime/src/tools/builtin.rs`
  - Register and dispatch the three new built-ins.
  - Keep existing Phase 1 tool behavior unchanged.
- `crates/agent-runtime/src/turn.rs`
  - Add turn-loop coverage proving Phase 2 built-ins are advertised/executable when enabled.
- `crates/agent-server/src/main.rs`
  - Read `GENERAL_AGENT_COMMAND_MODE=allowed` for development-only command mode.
- `docs/mvp-verification.md`
  - Append Phase 2 verification evidence after final verification passes.

Do not modify desktop UI files in Phase 2.

## Shared Result Shapes

`search_files` success data:

```json
{
  "path": ".",
  "pattern": "needle",
  "matches": [
    {
      "path": "src/lib.rs",
      "line": 3,
      "column": 7,
      "text": "let needle = true;"
    }
  ],
  "truncated": false,
  "engine": "rg"
}
```

`exec_command` success data:

```json
{
  "cmd": "printf hello",
  "cwd": ".",
  "exit_code": 0,
  "stdout": "hello",
  "stderr": "",
  "timed_out": false,
  "terminated_by_runtime": false
}
```

If stdout or stderr exceeds `RuntimeConfig.output_limit_bytes`, return a bounded success result with truncated text, `terminated_by_runtime: true`, and the corresponding metadata flags set. If the command times out, kill the child, wait for it, and return a failure result with code `timeout`.

`apply_patch` success data:

```json
{
  "changed_files": [
    {
      "path": "notes/example.txt",
      "action": "update",
      "added_lines": 1,
      "removed_lines": 1
    }
  ]
}
```

## Task 1: Command Capability Plumbing and Built-In Registration

**Files:**
- Modify: `crates/agent-runtime/src/tools/mod.rs`
- Modify: `crates/agent-runtime/src/tools/builtin.rs`
- Create: `crates/agent-runtime/src/tools/process.rs`
- Create: `crates/agent-runtime/src/tools/search.rs`
- Create: `crates/agent-runtime/src/tools/command.rs`
- Create: `crates/agent-runtime/src/tools/patch.rs`
- Test: `crates/agent-runtime/src/tools/mod.rs`
- Test: `crates/agent-runtime/src/tools/builtin.rs`

- [ ] **Step 1: Write failing permission and registration tests**

Add these tests to `crates/agent-runtime/src/tools/mod.rs`:

```rust
#[test]
fn command_permission_requires_workspace_write_and_command_allowed() {
    assert!(!permission_allowed(
        RuntimeMode::ReadOnly,
        CommandMode::Disabled,
        ToolPermission::ExecuteCommand
    ));
    assert!(!permission_allowed(
        RuntimeMode::ReadOnly,
        CommandMode::Allowed,
        ToolPermission::ExecuteCommand
    ));
    assert!(!permission_allowed(
        RuntimeMode::WorkspaceWrite,
        CommandMode::Disabled,
        ToolPermission::ExecuteCommand
    ));
    assert!(permission_allowed(
        RuntimeMode::WorkspaceWrite,
        CommandMode::Allowed,
        ToolPermission::ExecuteCommand
    ));
}

#[test]
fn runtime_config_can_enable_development_command_mode() {
    let workspace_root = PathBuf::from("/workspace");
    let cwd = workspace_root.join("project");
    let config = RuntimeConfig::workspace_write(workspace_root, cwd).with_command_mode(CommandMode::Allowed);

    assert_eq!(config.command_mode, CommandMode::Allowed);
}
```

Update the existing `read_only_blocks_workspace_writes` test so every call to `permission_allowed` passes `CommandMode::Disabled`.

Add this test to `crates/agent-runtime/src/tools/builtin.rs`:

```rust
#[tokio::test]
async fn definitions_include_exec_command_only_when_command_mode_allowed() {
    let root = unique_test_dir("command-definitions");
    std::fs::create_dir_all(&root).unwrap();

    let disabled_tools = BuiltInTools::new(RuntimeConfig::workspace_write(&root, &root));
    assert!(
        !disabled_tools
            .definitions()
            .iter()
            .any(|tool| tool.name == "exec_command")
    );

    let allowed_tools = BuiltInTools::new(
        RuntimeConfig::workspace_write(&root, &root).with_command_mode(CommandMode::Allowed),
    );
    assert!(
        allowed_tools
            .definitions()
            .iter()
            .any(|tool| tool.name == "exec_command")
    );

    remove_test_dir(root);
}

#[tokio::test]
async fn disabled_exec_command_returns_structured_failure_if_forced() {
    let root = unique_test_dir("command-disabled-forced");
    std::fs::create_dir_all(&root).unwrap();
    let tools = BuiltInTools::new(RuntimeConfig::workspace_write(&root, &root));

    let result = tools
        .execute("exec_command", "call-1", json!({ "cmd": "printf hello" }))
        .await;

    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "command_disabled");
    remove_test_dir(root);
}
```

- [ ] **Step 2: Run tests and verify they fail**

Run:

```bash
pixi run cargo test -p agent-runtime tools::tests::command_permission_requires_workspace_write_and_command_allowed
pixi run cargo test -p agent-runtime tools::builtin::tests::definitions_include_exec_command_only_when_command_mode_allowed
```

Expected:

- The first test fails because `CommandMode::Allowed`, `ToolPermission::ExecuteCommand`, and the new `permission_allowed` signature do not exist.
- The second test fails because `exec_command` is not registered.

- [ ] **Step 3: Implement command capability types**

In `crates/agent-runtime/src/tools/mod.rs`, change the enums and permission helper to this shape:

```rust
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum CommandMode {
    Disabled,
    Allowed,
}

impl RuntimeConfig {
    pub fn with_command_mode(mut self, command_mode: CommandMode) -> Self {
        self.command_mode = command_mode;
        self
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum ToolPermission {
    ReadWorkspace,
    WriteWorkspace,
    ExecuteCommand,
}

pub fn permission_allowed(
    mode: RuntimeMode,
    command_mode: CommandMode,
    permission: ToolPermission,
) -> bool {
    match permission {
        ToolPermission::ReadWorkspace => true,
        ToolPermission::WriteWorkspace => mode == RuntimeMode::WorkspaceWrite,
        ToolPermission::ExecuteCommand => {
            mode == RuntimeMode::WorkspaceWrite && command_mode == CommandMode::Allowed
        }
    }
}
```

Update `BuiltInTools::execute` to call:

```rust
if !permission_allowed(
    self.config.mode,
    self.config.command_mode,
    definition.permission,
) {
    let code = if definition.permission == ToolPermission::ExecuteCommand {
        "command_disabled"
    } else {
        "permission_denied"
    };
    return failure(
        name,
        call_id,
        code,
        "tool is not allowed in the current runtime mode",
        false,
        started,
    );
}
```

- [ ] **Step 4: Add module declarations and built-in dispatch placeholders**

At the top of `crates/agent-runtime/src/tools/mod.rs`, add:

```rust
pub mod command;
pub mod patch;
pub mod process;
pub mod search;
```

In `crates/agent-runtime/src/tools/builtin.rs`, import the new modules:

```rust
use super::{
    RuntimeConfig, ToolDefinition, ToolPermission, command, patch, path, permission_allowed,
    result::{ToolError, ToolResult, ToolResultMetadata},
    search,
};
```

Update `BuiltInTools::handles`:

```rust
matches!(
    name,
    CREATE_DIRECTORY
        | LIST_DIRECTORY
        | FILE_METADATA
        | READ_TEXT_FILE
        | WRITE_TEXT_FILE
        | search::SEARCH_FILES
        | command::EXEC_COMMAND
        | patch::APPLY_PATCH
)
```

Update `BuiltInTools::execute` dispatch:

```rust
search::SEARCH_FILES => search::execute(&self.config, call_id, arguments, started).await,
command::EXEC_COMMAND => command::execute(&self.config, call_id, arguments, started).await,
patch::APPLY_PATCH => patch::execute(&self.config, call_id, arguments, started).await,
```

Update `definitions()` into `definitions(config: &RuntimeConfig)` and append:

```rust
definitions.push(search::definition());
if permission_allowed(
    config.mode,
    config.command_mode,
    ToolPermission::ExecuteCommand,
) {
    definitions.push(command::definition());
}
definitions.push(patch::definition());
```

Keep existing Phase 1 tool definitions unchanged.

- [ ] **Step 5: Add temporary compiling module skeletons**

Create `crates/agent-runtime/src/tools/search.rs`:

```rust
use super::{RuntimeConfig, ToolDefinition, ToolPermission, result::ToolResult};
use serde_json::{Value, json};
use std::time::Instant;

pub const SEARCH_FILES: &str = "search_files";

pub fn definition() -> ToolDefinition {
    ToolDefinition {
        name: SEARCH_FILES.to_string(),
        description: "Search workspace text files for a pattern.".to_string(),
        permission: ToolPermission::ReadWorkspace,
        input_schema: json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string" },
                "path": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1 }
            },
            "required": ["pattern"],
            "additionalProperties": false
        }),
    }
}

pub async fn execute(
    _config: &RuntimeConfig,
    call_id: &str,
    _arguments: Value,
    started: Instant,
) -> anyhow::Result<ToolResult> {
    Ok(ToolResult::success(
        SEARCH_FILES,
        call_id,
        json!({
            "path": ".",
            "pattern": "",
            "matches": [],
            "truncated": false,
            "engine": "fallback"
        }),
        super::result::ToolResultMetadata {
            duration_ms: started.elapsed().as_millis() as u64,
            ..super::result::ToolResultMetadata::default()
        },
    ))
}
```

Create `crates/agent-runtime/src/tools/command.rs`:

```rust
use super::{
    RuntimeConfig, ToolDefinition, ToolPermission,
    result::{ToolError, ToolResult, ToolResultMetadata},
};
use serde_json::{Value, json};
use std::time::Instant;

pub const EXEC_COMMAND: &str = "exec_command";

pub fn definition() -> ToolDefinition {
    ToolDefinition {
        name: EXEC_COMMAND.to_string(),
        description: "Run a non-interactive command inside the workspace when development command mode is enabled.".to_string(),
        permission: ToolPermission::ExecuteCommand,
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
    }
}

pub async fn execute(
    _config: &RuntimeConfig,
    call_id: &str,
    _arguments: Value,
    started: Instant,
) -> anyhow::Result<ToolResult> {
    Ok(ToolResult::failure(
        EXEC_COMMAND,
        call_id,
        ToolError {
            code: "command_disabled".to_string(),
            message: "command execution is disabled".to_string(),
            retryable: false,
        },
        ToolResultMetadata {
            duration_ms: started.elapsed().as_millis() as u64,
            ..ToolResultMetadata::default()
        },
    ))
}
```

Create `crates/agent-runtime/src/tools/patch.rs`:

```rust
use super::{RuntimeConfig, ToolDefinition, ToolPermission, result::ToolResult};
use serde_json::{Value, json};
use std::time::Instant;

pub const APPLY_PATCH: &str = "apply_patch";

pub fn definition() -> ToolDefinition {
    ToolDefinition {
        name: APPLY_PATCH.to_string(),
        description: "Apply a Codex-style patch to workspace files.".to_string(),
        permission: ToolPermission::WriteWorkspace,
        input_schema: json!({
            "type": "object",
            "properties": {
                "patch": { "type": "string" }
            },
            "required": ["patch"],
            "additionalProperties": false
        }),
    }
}

pub async fn execute(
    _config: &RuntimeConfig,
    call_id: &str,
    _arguments: Value,
    started: Instant,
) -> anyhow::Result<ToolResult> {
    Ok(ToolResult::success(
        APPLY_PATCH,
        call_id,
        json!({ "changed_files": [] }),
        super::result::ToolResultMetadata {
            duration_ms: started.elapsed().as_millis() as u64,
            ..super::result::ToolResultMetadata::default()
        },
    ))
}
```

- [ ] **Step 6: Run capability tests**

Run:

```bash
pixi run cargo test -p agent-runtime tools::tests::command_permission_requires_workspace_write_and_command_allowed
pixi run cargo test -p agent-runtime tools::builtin::tests::definitions_include_exec_command_only_when_command_mode_allowed
pixi run cargo test -p agent-runtime tools::builtin::tests::disabled_exec_command_returns_structured_failure_if_forced
```

Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add crates/agent-runtime/src/tools/mod.rs crates/agent-runtime/src/tools/builtin.rs crates/agent-runtime/src/tools/process.rs crates/agent-runtime/src/tools/search.rs crates/agent-runtime/src/tools/command.rs crates/agent-runtime/src/tools/patch.rs
git commit -m "feat: add phase 2 tool registration plumbing"
```

## Task 2: Implement `search_files`

**Files:**
- Modify: `crates/agent-runtime/src/tools/search.rs`
- Test: `crates/agent-runtime/src/tools/search.rs`

- [ ] **Step 1: Write failing search tests**

Add these tests to `crates/agent-runtime/src/tools/search.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::RuntimeConfig;
    use serde_json::json;
    use std::path::PathBuf;

    #[tokio::test]
    async fn search_files_returns_structured_matches() {
        let root = unique_test_dir("search-matches");
        tokio::fs::create_dir_all(root.join("src")).await.unwrap();
        tokio::fs::write(root.join("src").join("lib.rs"), "fn main() {\n    let needle = true;\n}\n")
            .await
            .unwrap();
        tokio::fs::write(root.join("src").join("other.rs"), "nothing here\n")
            .await
            .unwrap();
        let config = RuntimeConfig::workspace_write(&root, &root);

        let result = execute(
            &config,
            "call-1",
            json!({ "pattern": "needle", "path": "src", "limit": 10 }),
            std::time::Instant::now(),
        )
        .await
        .unwrap();

        assert!(result.ok);
        let data = result.data.unwrap();
        assert_eq!(data["path"], "src");
        assert_eq!(data["pattern"], "needle");
        assert_eq!(data["truncated"], false);
        let matches = data["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0]["path"], "src/lib.rs");
        assert_eq!(matches[0]["line"], 2);
        assert_eq!(matches[0]["column"], 9);
        assert!(matches[0]["text"].as_str().unwrap().contains("needle"));
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn search_files_applies_limit_and_truncation_flag() {
        let root = unique_test_dir("search-limit");
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(root.join("a.txt"), "needle\nneedle\nneedle\n")
            .await
            .unwrap();
        let config = RuntimeConfig::workspace_write(&root, &root);

        let result = execute(
            &config,
            "call-1",
            json!({ "pattern": "needle", "limit": 2 }),
            std::time::Instant::now(),
        )
        .await
        .unwrap();

        assert!(result.ok);
        let data = result.data.unwrap();
        assert_eq!(data["matches"].as_array().unwrap().len(), 2);
        assert_eq!(data["truncated"], true);
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn search_files_rejects_workspace_escape() {
        let root = unique_test_dir("search-escape");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let config = RuntimeConfig::workspace_write(&root, &root);

        let result = execute(
            &config,
            "call-1",
            json!({ "pattern": "secret", "path": "../outside" }),
            std::time::Instant::now(),
        )
        .await
        .unwrap();

        assert!(!result.ok);
        assert_eq!(result.error.unwrap().code, "path_outside_workspace");
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn search_files_rejects_invalid_arguments() {
        let root = unique_test_dir("search-invalid-args");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let config = RuntimeConfig::workspace_write(&root, &root);

        let result = execute(
            &config,
            "call-1",
            json!({ "pattern": "", "limit": 0 }),
            std::time::Instant::now(),
        )
        .await
        .unwrap();

        assert!(!result.ok);
        assert_eq!(result.error.unwrap().code, "invalid_arguments");
        remove_test_dir(root).await;
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("generalagent-{name}-{}", uuid::Uuid::new_v4()))
    }

    async fn remove_test_dir(path: PathBuf) {
        if path.exists() {
            tokio::fs::remove_dir_all(path).await.unwrap();
        }
    }
}
```

- [ ] **Step 2: Run tests and verify they fail**

Run:

```bash
pixi run cargo test -p agent-runtime tools::search::tests -- --nocapture
```

Expected: tests fail because the skeleton returns no real matches.

- [ ] **Step 3: Implement argument parsing and fallback search**

In `crates/agent-runtime/src/tools/search.rs`, add:

```rust
#[derive(Debug)]
struct SearchArgs {
    pattern: String,
    path: String,
    limit: usize,
}

#[derive(Debug, Clone)]
struct SearchMatch {
    path: String,
    line: usize,
    column: usize,
    text: String,
}

fn parse_args(arguments: &Value) -> anyhow::Result<SearchArgs> {
    let pattern = arguments
        .get("pattern")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("invalid arguments: missing string field pattern"))?
        .to_string();
    let path = arguments
        .get("path")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(".")
        .to_string();
    let limit = match arguments.get("limit") {
        Some(value) => {
            let limit = value
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("invalid arguments: limit must be a positive integer"))?;
            if limit == 0 {
                anyhow::bail!("invalid arguments: limit must be a positive integer");
            }
            limit as usize
        }
        None => 100,
    };

    Ok(SearchArgs { pattern, path, limit })
}

fn metadata(started: Instant) -> super::result::ToolResultMetadata {
    super::result::ToolResultMetadata {
        duration_ms: started.elapsed().as_millis() as u64,
        ..super::result::ToolResultMetadata::default()
    }
}

fn failure(
    code: &str,
    message: impl Into<String>,
    call_id: &str,
    started: Instant,
) -> ToolResult {
    ToolResult::failure(
        SEARCH_FILES,
        call_id,
        super::result::ToolError {
            code: code.to_string(),
            message: message.into(),
            retryable: false,
        },
        metadata(started),
    )
}
```

Replace the skeleton `execute` body with:

```rust
let args = match parse_args(&arguments) {
    Ok(args) => args,
    Err(error) => {
        return Ok(failure("invalid_arguments", error.to_string(), call_id, started));
    }
};
let workspace_path = match super::path::resolve_existing_workspace_path(
    &config.workspace_root,
    &args.path,
) {
    Ok(path) => path,
    Err(error) => {
        return Ok(failure(error_code(&error.to_string()), error.to_string(), call_id, started));
    }
};

let (matches, truncated) =
    fallback_search(&workspace_path.absolute, &workspace_path.relative, &args.pattern, args.limit)
        .await?;

Ok(ToolResult::success(
    SEARCH_FILES,
    call_id,
    json!({
        "path": relative_path(&workspace_path.relative),
        "pattern": args.pattern,
        "matches": matches
            .into_iter()
            .map(|item| json!({
                "path": item.path,
                "line": item.line,
                "column": item.column,
                "text": item.text
            }))
            .collect::<Vec<_>>(),
        "truncated": truncated,
        "engine": "fallback"
    }),
    metadata(started),
))
```

Add fallback traversal:

```rust
async fn fallback_search(
    absolute: &std::path::Path,
    relative: &std::path::Path,
    pattern: &str,
    limit: usize,
) -> anyhow::Result<(Vec<SearchMatch>, bool)> {
    let mut files = Vec::new();
    collect_files(absolute, relative, &mut files).await?;
    files.sort_by(|left, right| left.1.cmp(&right.1));

    let mut matches = Vec::new();
    let mut truncated = false;
    for (absolute_file, relative_file) in files {
        let bytes = match tokio::fs::read(&absolute_file).await {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };
        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => continue,
        };
        for (line_index, line) in text.lines().enumerate() {
            if let Some(column_index) = line.find(pattern) {
                if matches.len() >= limit {
                    truncated = true;
                    return Ok((matches, truncated));
                }
                matches.push(SearchMatch {
                    path: relative_path(&relative_file),
                    line: line_index + 1,
                    column: column_index + 1,
                    text: line.to_string(),
                });
            }
        }
    }

    Ok((matches, truncated))
}

async fn collect_files(
    absolute: &std::path::Path,
    relative: &std::path::Path,
    files: &mut Vec<(std::path::PathBuf, std::path::PathBuf)>,
) -> anyhow::Result<()> {
    let metadata = tokio::fs::symlink_metadata(absolute).await?;
    if metadata.is_file() {
        files.push((absolute.to_path_buf(), relative.to_path_buf()));
        return Ok(());
    }
    if !metadata.is_dir() {
        return Ok(());
    }

    let mut entries = tokio::fs::read_dir(absolute).await?;
    while let Some(entry) = entries.next_entry().await? {
        let file_name = entry.file_name();
        let child_relative = relative.join(file_name);
        collect_files(&entry.path(), &child_relative, files).await?;
    }

    Ok(())
}
```

Add helpers:

```rust
fn error_code(message: &str) -> &'static str {
    if message.contains("outside workspace")
        || message.contains("parent traversal")
        || message.contains("empty workspace path")
    {
        "path_outside_workspace"
    } else if message.contains("No such file or directory")
        || message.contains("entity not found")
        || message.contains("failed to resolve workspace path")
    {
        "path_not_found"
    } else {
        "internal_error"
    }
}

fn relative_path(path: &std::path::Path) -> String {
    let value = path.to_string_lossy().to_string();
    if value.is_empty() {
        ".".to_string()
    } else {
        value
    }
}
```

- [ ] **Step 4: Add ripgrep fast path**

Add an `rg_search` helper and call it before fallback:

```rust
let (matches, truncated, engine) =
    match rg_search(&workspace_path.absolute, &workspace_path.relative, &args.pattern, args.limit)
        .await
    {
        Ok(Some((matches, truncated))) => (matches, truncated, "rg"),
        Ok(None) => {
            let (matches, truncated) = fallback_search(
                &workspace_path.absolute,
                &workspace_path.relative,
                &args.pattern,
                args.limit,
            )
            .await?;
            (matches, truncated, "fallback")
        }
        Err(error) => {
            return Ok(failure("search_failed", error.to_string(), call_id, started));
        }
    };
```

Implement `rg_search` without a shell:

```rust
async fn rg_search(
    absolute: &std::path::Path,
    relative: &std::path::Path,
    pattern: &str,
    limit: usize,
) -> anyhow::Result<Option<(Vec<SearchMatch>, bool)>> {
    let output = match tokio::process::Command::new("rg")
        .arg("--json")
        .arg("--color")
        .arg("never")
        .arg("--")
        .arg(pattern)
        .arg(absolute)
        .output()
        .await
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };

    if !output.status.success() && output.status.code() != Some(1) {
        anyhow::bail!(
            "rg failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let mut matches = Vec::new();
    let mut truncated = false;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let event: Value = match serde_json::from_str(line) {
            Ok(event) => event,
            Err(_) => continue,
        };
        if event.get("type").and_then(Value::as_str) != Some("match") {
            continue;
        }
        if matches.len() >= limit {
            truncated = true;
            break;
        }
        let data = &event["data"];
        let absolute_match_path = data["path"]["text"].as_str().unwrap_or_default();
        let match_path = std::path::Path::new(absolute_match_path);
        let display_path = match match_path.strip_prefix(absolute.parent().unwrap_or(absolute)) {
            Ok(path) if relative.as_os_str().is_empty() => path.to_path_buf(),
            Ok(path) => relative.join(path),
            Err(_) => match_path.to_path_buf(),
        };
        let line_text = data["lines"]["text"].as_str().unwrap_or_default().trim_end_matches('\n');
        let submatch = data["submatches"].as_array().and_then(|items| items.first());
        let column = submatch
            .and_then(|item| item["start"].as_u64())
            .map(|value| value as usize + 1)
            .unwrap_or(1);
        matches.push(SearchMatch {
            path: relative_path(&display_path),
            line: data["line_number"].as_u64().unwrap_or(1) as usize,
            column,
            text: line_text.to_string(),
        });
    }

    Ok(Some((matches, truncated)))
}
```

Ensure the final JSON uses the `engine` variable instead of a hard-coded value.

- [ ] **Step 5: Run search tests**

Run:

```bash
pixi run cargo test -p agent-runtime tools::search::tests -- --nocapture
```

Expected: all pass whether or not `rg` is installed.

- [ ] **Step 6: Commit**

```bash
git add crates/agent-runtime/src/tools/search.rs
git commit -m "feat: add workspace search tool"
```

## Task 3: Implement `exec_command`

**Files:**
- Modify: `crates/agent-runtime/src/tools/command.rs`
- Modify: `crates/agent-runtime/src/tools/process.rs`
- Modify: `crates/agent-server/src/main.rs`
- Test: `crates/agent-runtime/src/tools/command.rs`

- [ ] **Step 1: Write failing command tests**

Add these tests to `crates/agent-runtime/src/tools/command.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{CommandMode, RuntimeConfig};
    use serde_json::json;
    use std::path::PathBuf;

    #[tokio::test]
    async fn exec_command_runs_simple_command_inside_workspace() {
        let root = unique_test_dir("command-success");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let config =
            RuntimeConfig::workspace_write(&root, &root).with_command_mode(CommandMode::Allowed);

        let result = execute(
            &config,
            "call-1",
            json!({ "cmd": "printf hello" }),
            std::time::Instant::now(),
        )
        .await
        .unwrap();

        assert!(result.ok);
        let data = result.data.unwrap();
        assert_eq!(data["exit_code"], 0);
        assert_eq!(data["stdout"], "hello");
        assert_eq!(data["stderr"], "");
        assert_eq!(data["cwd"], ".");
        assert_eq!(data["timed_out"], false);
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn exec_command_reports_non_zero_exit_code() {
        let root = unique_test_dir("command-failure-exit");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let config =
            RuntimeConfig::workspace_write(&root, &root).with_command_mode(CommandMode::Allowed);

        let result = execute(
            &config,
            "call-1",
            json!({ "cmd": "printf nope >&2; exit 7" }),
            std::time::Instant::now(),
        )
        .await
        .unwrap();

        assert!(result.ok);
        let data = result.data.unwrap();
        assert_eq!(data["exit_code"], 7);
        assert_eq!(data["stderr"], "nope");
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn exec_command_rejects_command_when_disabled() {
        let root = unique_test_dir("command-disabled");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let config = RuntimeConfig::workspace_write(&root, &root);

        let result = execute(
            &config,
            "call-1",
            json!({ "cmd": "printf hello" }),
            std::time::Instant::now(),
        )
        .await
        .unwrap();

        assert!(!result.ok);
        assert_eq!(result.error.unwrap().code, "command_disabled");
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn exec_command_rejects_workspace_escape_cwd() {
        let root = unique_test_dir("command-cwd-escape");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let config =
            RuntimeConfig::workspace_write(&root, &root).with_command_mode(CommandMode::Allowed);

        let result = execute(
            &config,
            "call-1",
            json!({ "cmd": "pwd", "cwd": "../outside" }),
            std::time::Instant::now(),
        )
        .await
        .unwrap();

        assert!(!result.ok);
        assert_eq!(result.error.unwrap().code, "path_outside_workspace");
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn exec_command_blocks_denylisted_command_forms() {
        let root = unique_test_dir("command-deny");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let config =
            RuntimeConfig::workspace_write(&root, &root).with_command_mode(CommandMode::Allowed);

        let result = execute(
            &config,
            "call-1",
            json!({ "cmd": "git reset --hard" }),
            std::time::Instant::now(),
        )
        .await
        .unwrap();

        assert!(!result.ok);
        assert_eq!(result.error.unwrap().code, "command_denied");
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn exec_command_times_out_and_stops_child() {
        let root = unique_test_dir("command-timeout");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let mut config =
            RuntimeConfig::workspace_write(&root, &root).with_command_mode(CommandMode::Allowed);
        config.tool_timeout_ms = 50;

        let result = execute(
            &config,
            "call-1",
            json!({ "cmd": "sleep 0.2; touch late.txt", "timeout_ms": 25 }),
            std::time::Instant::now(),
        )
        .await
        .unwrap();

        assert!(!result.ok);
        assert_eq!(result.error.unwrap().code, "timeout");
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(!root.join("late.txt").exists());
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn exec_command_truncates_large_stdout() {
        let root = unique_test_dir("command-stdout-limit");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let mut config =
            RuntimeConfig::workspace_write(&root, &root).with_command_mode(CommandMode::Allowed);
        config.output_limit_bytes = 4;

        let result = execute(
            &config,
            "call-1",
            json!({ "cmd": "printf abcdef" }),
            std::time::Instant::now(),
        )
        .await
        .unwrap();

        assert!(result.ok);
        assert_eq!(result.data.as_ref().unwrap()["stdout"], "abcd");
        assert!(result.metadata.stdout_truncated);
        assert!(result.metadata.output_truncated);
        remove_test_dir(root).await;
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("generalagent-{name}-{}", uuid::Uuid::new_v4()))
    }

    async fn remove_test_dir(path: PathBuf) {
        if path.exists() {
            tokio::fs::remove_dir_all(path).await.unwrap();
        }
    }
}
```

- [ ] **Step 2: Run tests and verify they fail**

Run:

```bash
pixi run cargo test -p agent-runtime tools::command::tests -- --nocapture
```

Expected: tests fail because the skeleton never runs a command.

- [ ] **Step 3: Implement command parsing, deny rules, and environment**

In `crates/agent-runtime/src/tools/command.rs`, add:

```rust
#[derive(Debug)]
struct CommandArgs {
    cmd: String,
    cwd: String,
    timeout_ms: u64,
}

struct DenyRule {
    code: &'static str,
    needle: &'static str,
}

const DENY_RULES: &[DenyRule] = &[
    DenyRule { code: "git_reset_hard", needle: "git reset --hard" },
    DenyRule { code: "git_clean_force", needle: "git clean -fd" },
    DenyRule { code: "remove_root", needle: "rm -rf /" },
    DenyRule { code: "remove_current_tree", needle: "rm -rf ." },
    DenyRule { code: "sudo", needle: "sudo " },
    DenyRule { code: "shutdown", needle: "shutdown" },
    DenyRule { code: "reboot", needle: "reboot" },
    DenyRule { code: "mkfs", needle: "mkfs" },
];

fn parse_args(arguments: &Value, max_timeout_ms: u64) -> anyhow::Result<CommandArgs> {
    let cmd = arguments
        .get("cmd")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("invalid arguments: missing string field cmd"))?
        .to_string();
    let cwd = arguments
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(".")
        .to_string();
    let requested_timeout = arguments
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .unwrap_or(max_timeout_ms);
    let timeout_ms = requested_timeout.min(max_timeout_ms).max(1);

    Ok(CommandArgs { cmd, cwd, timeout_ms })
}

fn denied_command(cmd: &str) -> Option<&'static DenyRule> {
    let normalized = cmd.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    DENY_RULES
        .iter()
        .find(|rule| normalized.contains(rule.needle))
}
```

Add failure helpers:

```rust
fn metadata(started: Instant) -> ToolResultMetadata {
    ToolResultMetadata {
        duration_ms: started.elapsed().as_millis() as u64,
        ..ToolResultMetadata::default()
    }
}

fn failure(
    code: &str,
    message: impl Into<String>,
    call_id: &str,
    started: Instant,
) -> ToolResult {
    ToolResult::failure(
        EXEC_COMMAND,
        call_id,
        ToolError {
            code: code.to_string(),
            message: message.into(),
            retryable: code == "timeout",
        },
        metadata(started),
    )
}

fn path_error_code(message: &str) -> &'static str {
    if message.contains("outside workspace")
        || message.contains("parent traversal")
        || message.contains("empty workspace path")
    {
        "path_outside_workspace"
    } else if message.contains("No such file or directory")
        || message.contains("entity not found")
        || message.contains("failed to resolve workspace path")
    {
        "path_not_found"
    } else {
        "internal_error"
    }
}
```

- [ ] **Step 4: Implement shared limited child output**

Add this implementation to `crates/agent-runtime/src/tools/process.rs`:

```rust
use tokio::io::{AsyncRead, AsyncReadExt};

pub struct LimitedChildOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

struct LimitedOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

pub async fn read_limited_child_output(
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    output_limit_bytes: usize,
) -> anyhow::Result<LimitedChildOutput> {
    let stdout_future = read_limited_stream(stdout, output_limit_bytes);
    let stderr_future = read_limited_stream(stderr, output_limit_bytes);
    tokio::pin!(stdout_future);
    tokio::pin!(stderr_future);

    let mut stdout_output: Option<LimitedOutput> = None;
    let mut stderr_output: Option<LimitedOutput> = None;

    while stdout_output.is_none() || stderr_output.is_none() {
        tokio::select! {
            output = &mut stdout_future, if stdout_output.is_none() => {
                stdout_output = Some(output?);
            }
            output = &mut stderr_future, if stderr_output.is_none() => {
                stderr_output = Some(output?);
            }
        }

        if stdout_output.as_ref().is_some_and(|output| output.truncated)
            || stderr_output.as_ref().is_some_and(|output| output.truncated)
        {
            break;
        }
    }

    let stdout = stdout_output.unwrap_or(LimitedOutput { bytes: Vec::new(), truncated: false });
    let stderr = stderr_output.unwrap_or(LimitedOutput { bytes: Vec::new(), truncated: false });
    Ok(LimitedChildOutput {
        stdout: stdout.bytes,
        stderr: stderr.bytes,
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
    })
}

async fn read_limited_stream(
    mut stream: impl AsyncRead + Unpin,
    output_limit_bytes: usize,
) -> anyhow::Result<LimitedOutput> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    let hard_limit = output_limit_bytes.saturating_add(1);

    loop {
        let remaining = hard_limit.saturating_sub(bytes.len());
        if remaining == 0 {
            return Ok(LimitedOutput { bytes, truncated: true });
        }

        let read_len = remaining.min(buffer.len());
        let read = stream.read(&mut buffer[..read_len]).await?;
        if read == 0 {
            return Ok(LimitedOutput { bytes, truncated: false });
        }

        bytes.extend_from_slice(&buffer[..read]);
        if bytes.len() > output_limit_bytes {
            bytes.truncate(output_limit_bytes);
            return Ok(LimitedOutput { bytes, truncated: true });
        }
    }
}
```

- [ ] **Step 5: Implement shell execution**

Replace the skeleton `execute` body with:

```rust
if config.command_mode != super::CommandMode::Allowed {
    return Ok(failure(
        "command_disabled",
        "command execution is disabled",
        call_id,
        started,
    ));
}
if config.mode != super::RuntimeMode::WorkspaceWrite {
    return Ok(failure(
        "permission_denied",
        "command execution requires workspace_write runtime mode",
        call_id,
        started,
    ));
}

let args = match parse_args(&arguments, config.tool_timeout_ms) {
    Ok(args) => args,
    Err(error) => return Ok(failure("invalid_arguments", error.to_string(), call_id, started)),
};
if let Some(rule) = denied_command(&args.cmd) {
    return Ok(failure(
        "command_denied",
        format!("command denied by rule {}", rule.code),
        call_id,
        started,
    ));
}

let workspace_cwd = match super::path::resolve_existing_workspace_path(
    &config.workspace_root,
    &args.cwd,
) {
    Ok(path) => path,
    Err(error) => {
        return Ok(failure(
            path_error_code(&error.to_string()),
            error.to_string(),
            call_id,
            started,
        ));
    }
};

let mut child = shell_command(&args.cmd)
    .current_dir(&workspace_cwd.absolute)
    .env_clear()
    .envs(command_environment())
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped())
    .kill_on_drop(true)
    .spawn()?;
let stdout = child
    .stdout
    .take()
    .ok_or_else(|| anyhow::anyhow!("command stdout unavailable"))?;
let stderr = child
    .stderr
    .take()
    .ok_or_else(|| anyhow::anyhow!("command stderr unavailable"))?;

let output_future = super::process::read_limited_child_output(
    stdout,
    stderr,
    config.output_limit_bytes,
);
let output = match tokio::time::timeout(
    std::time::Duration::from_millis(args.timeout_ms),
    output_future,
)
.await
{
    Ok(result) => result?,
    Err(_) => {
        let _ = child.kill().await;
        let _ = child.wait().await;
        return Ok(failure("timeout", "command execution timed out", call_id, started));
    }
};
let output_was_truncated = output.stdout_truncated || output.stderr_truncated;
let status = if output_was_truncated {
    child.kill().await?;
    let _ = child.wait().await;
    None
} else {
    Some(child.wait().await?)
};

let mut result_metadata = metadata(started);
result_metadata.stdout_truncated = output.stdout_truncated;
result_metadata.stderr_truncated = output.stderr_truncated;
result_metadata.output_truncated = output.stdout_truncated || output.stderr_truncated;

Ok(ToolResult::success(
    EXEC_COMMAND,
    call_id,
    json!({
        "cmd": args.cmd,
        "cwd": relative_path(&workspace_cwd.relative),
        "exit_code": status.and_then(|status| status.code()),
        "stdout": String::from_utf8_lossy(&output.stdout).to_string(),
        "stderr": String::from_utf8_lossy(&output.stderr).to_string(),
        "timed_out": false,
        "terminated_by_runtime": output.stdout_truncated || output.stderr_truncated
    }),
    result_metadata,
))
```

Add shell/environment helpers:

```rust
fn shell_command(cmd: &str) -> tokio::process::Command {
    #[cfg(windows)]
    {
        let mut command = tokio::process::Command::new("cmd");
        command.arg("/C").arg(cmd);
        command
    }
    #[cfg(not(windows))]
    {
        let mut command = tokio::process::Command::new("sh");
        command.arg("-c").arg(cmd);
        command
    }
}

fn command_environment() -> Vec<(String, String)> {
    const ALLOWLIST: &[&str] = &[
        "PATH", "HOME", "TMPDIR", "TEMP", "TMP", "USER", "SHELL", "LANG", "LC_ALL",
    ];
    ALLOWLIST
        .iter()
        .filter_map(|name| std::env::var(name).ok().map(|value| ((*name).to_string(), value)))
        .collect()
}

fn relative_path(path: &std::path::Path) -> String {
    let value = path.to_string_lossy().to_string();
    if value.is_empty() {
        ".".to_string()
    } else {
        value
    }
}
```

- [ ] **Step 6: Wire server environment command mode**

In `crates/agent-server/src/main.rs`, update `runtime_config_from_env`:

```rust
fn runtime_config_from_env() -> RuntimeConfig {
    let workspace_root = std::env::var("GENERAL_AGENT_WORKSPACE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let command_mode = match std::env::var("GENERAL_AGENT_COMMAND_MODE")
        .unwrap_or_else(|_| "disabled".into())
        .as_str()
    {
        "allowed" => agent_runtime::tools::CommandMode::Allowed,
        _ => agent_runtime::tools::CommandMode::Disabled,
    };

    RuntimeConfig::workspace_write(workspace_root.clone(), workspace_root)
        .with_command_mode(command_mode)
}
```

- [ ] **Step 7: Run command tests**

Run:

```bash
pixi run cargo test -p agent-runtime tools::command::tests -- --nocapture
pixi run cargo test -p agent-runtime tools::builtin::tests::definitions_include_exec_command_only_when_command_mode_allowed
pixi run cargo test -p agent-runtime tools::builtin::tests::disabled_exec_command_returns_structured_failure_if_forced
```

Expected: all pass.

- [ ] **Step 8: Commit**

```bash
git add crates/agent-runtime/src/tools/command.rs crates/agent-runtime/src/tools/process.rs crates/agent-server/src/main.rs
git commit -m "feat: add development command tool"
```

## Task 4: Implement `apply_patch`

**Files:**
- Modify: `crates/agent-runtime/src/tools/patch.rs`
- Test: `crates/agent-runtime/src/tools/patch.rs`

- [ ] **Step 1: Write failing patch tests**

Add these tests to `crates/agent-runtime/src/tools/patch.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::RuntimeConfig;
    use serde_json::json;
    use std::path::PathBuf;

    #[tokio::test]
    async fn apply_patch_adds_file_inside_workspace() {
        let root = unique_test_dir("patch-add-file");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let config = RuntimeConfig::workspace_write(&root, &root);

        let result = execute(
            &config,
            "call-1",
            json!({
                "patch": "*** Begin Patch\n*** Add File: notes/hello.txt\n+hello\n+world\n*** End Patch\n"
            }),
            std::time::Instant::now(),
        )
        .await
        .unwrap();

        assert!(result.ok);
        assert_eq!(
            tokio::fs::read_to_string(root.join("notes").join("hello.txt"))
                .await
                .unwrap(),
            "hello\nworld\n"
        );
        assert_eq!(result.data.unwrap()["changed_files"][0]["action"], "add");
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn apply_patch_updates_file_with_context_hunk() {
        let root = unique_test_dir("patch-update-file");
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(root.join("hello.txt"), "alpha\nbeta\ngamma\n")
            .await
            .unwrap();
        let config = RuntimeConfig::workspace_write(&root, &root);

        let result = execute(
            &config,
            "call-1",
            json!({
                "patch": "*** Begin Patch\n*** Update File: hello.txt\n@@\n alpha\n-beta\n+bravo\n gamma\n*** End Patch\n"
            }),
            std::time::Instant::now(),
        )
        .await
        .unwrap();

        assert!(result.ok);
        assert_eq!(
            tokio::fs::read_to_string(root.join("hello.txt")).await.unwrap(),
            "alpha\nbravo\ngamma\n"
        );
        assert_eq!(result.data.unwrap()["changed_files"][0]["action"], "update");
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn apply_patch_deletes_file_inside_workspace() {
        let root = unique_test_dir("patch-delete-file");
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(root.join("obsolete.txt"), "remove me\n")
            .await
            .unwrap();
        let config = RuntimeConfig::workspace_write(&root, &root);

        let result = execute(
            &config,
            "call-1",
            json!({
                "patch": "*** Begin Patch\n*** Delete File: obsolete.txt\n*** End Patch\n"
            }),
            std::time::Instant::now(),
        )
        .await
        .unwrap();

        assert!(result.ok);
        assert!(!root.join("obsolete.txt").exists());
        assert_eq!(result.data.unwrap()["changed_files"][0]["action"], "delete");
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn apply_patch_rejects_outside_workspace_add() {
        let root = unique_test_dir("patch-outside-add");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let config = RuntimeConfig::workspace_write(&root, &root);

        let result = execute(
            &config,
            "call-1",
            json!({
                "patch": "*** Begin Patch\n*** Add File: ../outside.txt\n+nope\n*** End Patch\n"
            }),
            std::time::Instant::now(),
        )
        .await
        .unwrap();

        assert!(!result.ok);
        assert_eq!(result.error.unwrap().code, "path_outside_workspace");
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn apply_patch_rejects_hunk_that_does_not_match() {
        let root = unique_test_dir("patch-hunk-mismatch");
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(root.join("hello.txt"), "alpha\nbeta\n")
            .await
            .unwrap();
        let config = RuntimeConfig::workspace_write(&root, &root);

        let result = execute(
            &config,
            "call-1",
            json!({
                "patch": "*** Begin Patch\n*** Update File: hello.txt\n@@\n-missing\n+present\n*** End Patch\n"
            }),
            std::time::Instant::now(),
        )
        .await
        .unwrap();

        assert!(!result.ok);
        assert_eq!(result.error.unwrap().code, "patch_apply_failed");
        assert_eq!(
            tokio::fs::read_to_string(root.join("hello.txt")).await.unwrap(),
            "alpha\nbeta\n"
        );
        remove_test_dir(root).await;
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("generalagent-{name}-{}", uuid::Uuid::new_v4()))
    }

    async fn remove_test_dir(path: PathBuf) {
        if path.exists() {
            tokio::fs::remove_dir_all(path).await.unwrap();
        }
    }
}
```

- [ ] **Step 2: Run tests and verify they fail**

Run:

```bash
pixi run cargo test -p agent-runtime tools::patch::tests -- --nocapture
```

Expected: tests fail because the skeleton does not apply patches.

- [ ] **Step 3: Implement parser types and parse function**

In `crates/agent-runtime/src/tools/patch.rs`, add:

```rust
#[derive(Debug)]
enum PatchOperation {
    Add { path: String, lines: Vec<String> },
    Update { path: String, hunks: Vec<Hunk> },
    Delete { path: String },
}

#[derive(Debug)]
struct Hunk {
    lines: Vec<HunkLine>,
}

#[derive(Debug)]
enum HunkLine {
    Context(String),
    Remove(String),
    Add(String),
}

fn parse_patch(input: &str) -> anyhow::Result<Vec<PatchOperation>> {
    let lines: Vec<&str> = input.lines().collect();
    if lines.first() != Some(&"*** Begin Patch") || lines.last() != Some(&"*** End Patch") {
        anyhow::bail!("invalid patch: missing begin or end marker");
    }

    let mut operations = Vec::new();
    let mut index = 1usize;
    while index + 1 < lines.len() {
        let line = lines[index];
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            index += 1;
            let mut add_lines = Vec::new();
            while index + 1 < lines.len() && !lines[index].starts_with("*** ") {
                let Some(content) = lines[index].strip_prefix('+') else {
                    anyhow::bail!("invalid patch: add file lines must start with +");
                };
                add_lines.push(content.to_string());
                index += 1;
            }
            operations.push(PatchOperation::Add {
                path: path.to_string(),
                lines: add_lines,
            });
        } else if let Some(path) = line.strip_prefix("*** Update File: ") {
            index += 1;
            let mut hunks = Vec::new();
            while index + 1 < lines.len() && lines[index] == "@@" {
                index += 1;
                let mut hunk_lines = Vec::new();
                while index + 1 < lines.len()
                    && !lines[index].starts_with("*** ")
                    && lines[index] != "@@"
                {
                    let raw = lines[index];
                    let (prefix, content) = raw.split_at(1);
                    match prefix {
                        " " => hunk_lines.push(HunkLine::Context(content.to_string())),
                        "-" => hunk_lines.push(HunkLine::Remove(content.to_string())),
                        "+" => hunk_lines.push(HunkLine::Add(content.to_string())),
                        _ => anyhow::bail!("invalid patch: hunk line has invalid prefix"),
                    }
                    index += 1;
                }
                hunks.push(Hunk { lines: hunk_lines });
            }
            if hunks.is_empty() {
                anyhow::bail!("invalid patch: update file requires at least one hunk");
            }
            operations.push(PatchOperation::Update {
                path: path.to_string(),
                hunks,
            });
        } else if let Some(path) = line.strip_prefix("*** Delete File: ") {
            operations.push(PatchOperation::Delete {
                path: path.to_string(),
            });
            index += 1;
        } else {
            anyhow::bail!("invalid patch: unknown operation marker");
        }
    }

    if operations.is_empty() {
        anyhow::bail!("invalid patch: no operations");
    }
    Ok(operations)
}
```

- [ ] **Step 4: Implement update hunk application**

Add:

```rust
fn apply_hunks(original: &str, hunks: &[Hunk]) -> anyhow::Result<(String, usize, usize)> {
    let had_trailing_newline = original.ends_with('\n');
    let mut lines: Vec<String> = original.lines().map(ToString::to_string).collect();
    let mut added = 0usize;
    let mut removed = 0usize;

    for hunk in hunks {
        let expected: Vec<String> = hunk
            .lines
            .iter()
            .filter_map(|line| match line {
                HunkLine::Context(text) | HunkLine::Remove(text) => Some(text.clone()),
                HunkLine::Add(_) => None,
            })
            .collect();
        let replacement: Vec<String> = hunk
            .lines
            .iter()
            .filter_map(|line| match line {
                HunkLine::Context(text) | HunkLine::Add(text) => Some(text.clone()),
                HunkLine::Remove(_) => None,
            })
            .collect();

        if expected.is_empty() {
            anyhow::bail!("patch apply failed: update hunk requires context or removal lines");
        }
        let Some(position) = find_subsequence(&lines, &expected) else {
            anyhow::bail!("patch apply failed: hunk context did not match");
        };

        removed += hunk
            .lines
            .iter()
            .filter(|line| matches!(line, HunkLine::Remove(_)))
            .count();
        added += hunk
            .lines
            .iter()
            .filter(|line| matches!(line, HunkLine::Add(_)))
            .count();
        lines.splice(position..position + expected.len(), replacement);
    }

    let mut output = lines.join("\n");
    if had_trailing_newline || !output.is_empty() {
        output.push('\n');
    }
    Ok((output, added, removed))
}

fn find_subsequence(lines: &[String], expected: &[String]) -> Option<usize> {
    lines
        .windows(expected.len())
        .position(|window| window == expected)
}
```

- [ ] **Step 5: Implement patch execution**

Replace the skeleton `execute` body with:

```rust
let Some(patch_text) = arguments.get("patch").and_then(Value::as_str).filter(|value| !value.is_empty()) else {
    return Ok(failure(
        "invalid_arguments",
        "invalid arguments: missing string field patch",
        call_id,
        started,
    ));
};
let operations = match parse_patch(patch_text) {
    Ok(operations) => operations,
    Err(error) => return Ok(failure("invalid_patch", error.to_string(), call_id, started)),
};

let mut summaries = Vec::new();
for operation in operations {
    match operation {
        PatchOperation::Add { path, lines } => {
            let workspace_path = match super::path::resolve_workspace_output_path(&config.workspace_root, &path) {
                Ok(path) => path,
                Err(error) => return Ok(path_failure(error, call_id, started)),
            };
            if workspace_path.absolute.exists() {
                return Ok(failure("path_exists", "refusing to add file that already exists", call_id, started));
            }
            if let Some(parent) = workspace_path.absolute.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            let content = if lines.is_empty() {
                String::new()
            } else {
                format!("{}\n", lines.join("\n"))
            };
            tokio::fs::write(&workspace_path.absolute, content).await?;
            summaries.push(json!({
                "path": relative_path(&workspace_path.relative),
                "action": "add",
                "added_lines": lines.len(),
                "removed_lines": 0
            }));
        }
        PatchOperation::Update { path, hunks } => {
            let workspace_path = match super::path::resolve_existing_workspace_path(&config.workspace_root, &path) {
                Ok(path) => path,
                Err(error) => return Ok(path_failure(error, call_id, started)),
            };
            let original = match tokio::fs::read_to_string(&workspace_path.absolute).await {
                Ok(text) => text,
                Err(error) => return Ok(failure("path_not_text", error.to_string(), call_id, started)),
            };
            let (updated, added, removed) = match apply_hunks(&original, &hunks) {
                Ok(result) => result,
                Err(error) => return Ok(failure("patch_apply_failed", error.to_string(), call_id, started)),
            };
            tokio::fs::write(&workspace_path.absolute, updated).await?;
            summaries.push(json!({
                "path": relative_path(&workspace_path.relative),
                "action": "update",
                "added_lines": added,
                "removed_lines": removed
            }));
        }
        PatchOperation::Delete { path } => {
            let workspace_path = match super::path::resolve_existing_workspace_path(&config.workspace_root, &path) {
                Ok(path) => path,
                Err(error) => return Ok(path_failure(error, call_id, started)),
            };
            tokio::fs::remove_file(&workspace_path.absolute).await?;
            summaries.push(json!({
                "path": relative_path(&workspace_path.relative),
                "action": "delete",
                "added_lines": 0,
                "removed_lines": 0
            }));
        }
    }
}

Ok(ToolResult::success(
    APPLY_PATCH,
    call_id,
    json!({ "changed_files": summaries }),
    metadata(started),
))
```

Add helpers:

```rust
fn metadata(started: Instant) -> super::result::ToolResultMetadata {
    super::result::ToolResultMetadata {
        duration_ms: started.elapsed().as_millis() as u64,
        ..super::result::ToolResultMetadata::default()
    }
}

fn failure(
    code: &str,
    message: impl Into<String>,
    call_id: &str,
    started: Instant,
) -> ToolResult {
    ToolResult::failure(
        APPLY_PATCH,
        call_id,
        super::result::ToolError {
            code: code.to_string(),
            message: message.into(),
            retryable: false,
        },
        metadata(started),
    )
}

fn path_failure(error: anyhow::Error, call_id: &str, started: Instant) -> ToolResult {
    let message = error.to_string();
    failure(path_error_code(&message), message, call_id, started)
}

fn path_error_code(message: &str) -> &'static str {
    if message.contains("outside workspace")
        || message.contains("parent traversal")
        || message.contains("empty workspace path")
    {
        "path_outside_workspace"
    } else if message.contains("No such file or directory")
        || message.contains("entity not found")
        || message.contains("failed to resolve workspace path")
    {
        "path_not_found"
    } else {
        "internal_error"
    }
}

fn relative_path(path: &std::path::Path) -> String {
    let value = path.to_string_lossy().to_string();
    if value.is_empty() {
        ".".to_string()
    } else {
        value
    }
}
```

- [ ] **Step 6: Run patch tests**

Run:

```bash
pixi run cargo test -p agent-runtime tools::patch::tests -- --nocapture
```

Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add crates/agent-runtime/src/tools/patch.rs
git commit -m "feat: add workspace patch tool"
```

## Task 5: Turn-Loop Integration Tests and Runtime Documentation

**Files:**
- Modify: `crates/agent-runtime/src/turn.rs`
- Modify: `docs/mvp-verification.md`

- [ ] **Step 1: Write failing turn-loop Phase 2 tests**

Add a `FakePhaseTwoModel` to `crates/agent-runtime/src/turn.rs` tests:

```rust
struct FakePhaseTwoModel {
    calls: AtomicUsize,
    tool_name: &'static str,
    arguments: serde_json::Value,
    requests: Mutex<Vec<model_gateway::responses::GatewayRequest>>,
}

#[async_trait]
impl ModelClient for FakePhaseTwoModel {
    async fn stream(
        &self,
        request: model_gateway::responses::GatewayRequest,
    ) -> anyhow::Result<ModelEventStream> {
        self.requests.lock().unwrap().push(request);
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let events = if call == 0 {
            vec![
                Ok(GatewayEvent::ToolCall {
                    call_id: "call-1".into(),
                    name: self.tool_name.into(),
                    arguments: self.arguments.clone(),
                }),
                Ok(GatewayEvent::Completed),
            ]
        } else {
            vec![
                Ok(GatewayEvent::TextDelta {
                    text: "done".into(),
                }),
                Ok(GatewayEvent::Completed),
            ]
        };
        Ok(Box::pin(stream::iter(events)))
    }
}
```

Add tests:

```rust
#[tokio::test]
async fn phase_two_search_files_executes_through_turn_loop() {
    let workspace = unique_test_dir("turn-search-files");
    fs::create_dir_all(&workspace).unwrap();
    fs::write(workspace.join("notes.txt"), "find me\n").unwrap();
    let skills = SkillRegistry::load_development(empty_skills_root(&workspace)).await.unwrap();
    let config = RuntimeConfig::workspace_write(workspace.clone(), workspace.clone());
    let runner = TurnRunner::new_with_config(
        FakePhaseTwoModel {
            calls: AtomicUsize::new(0),
            tool_name: "search_files",
            arguments: serde_json::json!({ "pattern": "find" }),
            requests: Mutex::new(Vec::new()),
        },
        skills,
        config,
    );

    let events = runner.run("search for find").await.unwrap();

    let result = tool_result(&events);
    assert_eq!(result["tool"], "search_files");
    assert_eq!(result["data"]["matches"][0]["path"], "notes.txt");
    remove_test_dir(workspace);
}

#[tokio::test]
async fn phase_two_exec_command_is_advertised_only_when_allowed() {
    let workspace = unique_test_dir("turn-command-advertise");
    fs::create_dir_all(&workspace).unwrap();
    let skills = SkillRegistry::load_development(empty_skills_root(&workspace)).await.unwrap();
    let config =
        RuntimeConfig::workspace_write(workspace.clone(), workspace.clone())
            .with_command_mode(crate::tools::CommandMode::Allowed);
    let model = FakePhaseTwoModel {
        calls: AtomicUsize::new(0),
        tool_name: "exec_command",
        arguments: serde_json::json!({ "cmd": "printf hello" }),
        requests: Mutex::new(Vec::new()),
    };
    let runner = TurnRunner::new_with_config(model, skills, config);

    let events = runner.run("run printf").await.unwrap();

    let result = tool_result(&events);
    assert_eq!(result["tool"], "exec_command");
    assert_eq!(result["data"]["stdout"], "hello");
    remove_test_dir(workspace);
}
```

If helpers such as `unique_test_dir`, `remove_test_dir`, `empty_skills_root`, or `tool_result` already exist in the test module, reuse them. Otherwise add:

```rust
fn tool_result(events: &[RuntimeEvent]) -> serde_json::Value {
    events
        .iter()
        .find_map(|event| match event {
            RuntimeEvent::ToolCallFinished { result, .. } => Some(result.clone()),
            _ => None,
        })
        .expect("tool result event should be present")
}
```

- [ ] **Step 2: Run turn-loop tests and verify they fail if integration is incomplete**

Run:

```bash
pixi run cargo test -p agent-runtime turn::tests::phase_two_search_files_executes_through_turn_loop
pixi run cargo test -p agent-runtime turn::tests::phase_two_exec_command_is_advertised_only_when_allowed
```

Expected: pass if Tasks 1-4 are complete; otherwise fail at the missing integration point.

- [ ] **Step 3: Update verification document**

After final verification commands pass in Task 6, append a new section to `docs/mvp-verification.md`:

```markdown
## Codex-Like Runtime Phase 2 Verification

Date: 2026-06-27

Scope:
- Added `search_files`, `exec_command`, and `apply_patch` built-in runtime tools.
- Kept `exec_command` development-only through `command_disabled` / `command_allowed` runtime mode.
- Added command deny rules, workspace cwd validation, timeout handling, and bounded stdout/stderr.
- Added workspace-scoped patch parsing and outside-workspace rejection.

Verification:
- `pixi run cargo test --workspace`: PASS
- `pixi run cargo clippy --workspace --all-targets -- -D warnings`: PASS
- `pixi run cargo fmt --all --check`: PASS
- `git diff --check HEAD`: PASS
- Source line count check: PASS, no edited or new source file exceeds 1000 physical lines.

Notes:
- `exec_command` is intentionally non-interactive in Phase 2.
- `apply_patch` supports add, update, and delete operations with context hunks; moves and binary patches remain out of scope.
- Command execution remains development-only. Approval workflows and stronger sandbox profiles are later phases.
```

Only record `PASS` after the command actually passes.

- [ ] **Step 4: Commit**

```bash
git add crates/agent-runtime/src/turn.rs docs/mvp-verification.md
git commit -m "test: cover phase 2 runtime tools"
```

## Task 6: Full Verification and Final Review

**Files:**
- Modify: `docs/superpowers/plans/2026-06-27-codex-like-runtime-phase-2.md`

- [ ] **Step 1: Run focused tests**

Run:

```bash
pixi run cargo test -p agent-runtime tools::search::tests -- --nocapture
pixi run cargo test -p agent-runtime tools::command::tests -- --nocapture
pixi run cargo test -p agent-runtime tools::patch::tests -- --nocapture
pixi run cargo test -p agent-runtime turn::tests::phase_two_search_files_executes_through_turn_loop
pixi run cargo test -p agent-runtime turn::tests::phase_two_exec_command_is_advertised_only_when_allowed
```

Expected: all pass.

- [ ] **Step 2: Run full workspace verification**

Run:

```bash
pixi run cargo test --workspace
pixi run cargo clippy --workspace --all-targets -- -D warnings
pixi run cargo fmt --all --check
git diff --check HEAD
wc -l crates/agent-runtime/src/tools/*.rs crates/agent-runtime/src/*.rs crates/model-gateway/src/*.rs crates/agent-server/src/*.rs | sort -n
```

Expected:

- All tests pass.
- Clippy passes with `-D warnings`.
- Formatting check passes.
- `git diff --check HEAD` reports no whitespace errors.
- No source file in the output exceeds 1000 physical lines.

- [ ] **Step 3: Request spec compliance review**

Dispatch a fresh reviewer subagent with:

```text
Review the completed GeneralAgent Codex-like runtime Phase 2 implementation against docs/superpowers/specs/2026-06-27-codex-like-runtime-migration-design.md and docs/superpowers/plans/2026-06-27-codex-like-runtime-phase-2.md.

Focus only on spec compliance:
- search_files behavior.
- exec_command command mode, cwd validation, timeout, output limits, deny rules.
- apply_patch workspace safety and minimal grammar.
- required tests and verification docs.
- no edited/new source file over 1000 lines.

Return Critical/Important/Minor findings with file paths and line references. If compliant, say so clearly.
```

Fix all Critical and Important issues before moving on.

- [ ] **Step 4: Request quality review**

Dispatch a fresh reviewer subagent with:

```text
Review the completed GeneralAgent Codex-like runtime Phase 2 implementation for code quality.

Focus on:
- Rust correctness and async process safety.
- Error codes and result envelope consistency.
- Path validation and symlink escape risks.
- Test reliability across macOS/Linux.
- Duplication that creates real maintenance risk.
- source files under 1000 physical lines.

Return Critical/Important/Minor findings with file paths and line references. If quality is acceptable, say so clearly.
```

Fix all Critical and Important issues before finalizing.

- [ ] **Step 5: Update this plan with verification evidence**

Append a final section:

```markdown
## Codex-Like Runtime Phase 2 Completion Evidence

Completed: 2026-06-27

Commits:
- `<sha> feat: add phase 2 tool registration plumbing`
- `<sha> feat: add workspace search tool`
- `<sha> feat: add development command tool`
- `<sha> feat: add workspace patch tool`
- `<sha> test: cover phase 2 runtime tools`
- `<sha> docs: record codex-like runtime phase 2 verification`

Verification:
- `pixi run cargo test --workspace`: PASS
- `pixi run cargo clippy --workspace --all-targets -- -D warnings`: PASS
- `pixi run cargo fmt --all --check`: PASS
- `git diff --check HEAD`: PASS
- Line count check: PASS, no edited/new source file exceeds 1000 physical lines.

Review:
- Spec compliance reviewer: PASS
- Code quality reviewer: PASS
```

Replace every `<sha>` with the actual commit SHA. Do not write `PASS` until the command or review actually passed.

- [ ] **Step 6: Commit final verification docs**

```bash
git add docs/superpowers/plans/2026-06-27-codex-like-runtime-phase-2.md docs/mvp-verification.md
git commit -m "docs: record codex-like runtime phase 2 verification"
```

## Phase 2 Acceptance Checklist

- [ ] `search_files` can search workspace files without shell fallback.
- [ ] `search_files` refuses paths outside the workspace.
- [ ] `exec_command` is not advertised by default.
- [ ] `exec_command` returns `command_disabled` if forced while disabled.
- [ ] `exec_command` runs simple workspace commands when `CommandMode::Allowed`.
- [ ] `exec_command` validates `cwd` inside the workspace.
- [ ] `exec_command` applies table-driven deny rules.
- [ ] `exec_command` times out and stops the direct child process.
- [ ] `exec_command` returns bounded stdout/stderr and truncation metadata.
- [ ] `apply_patch` can add, update, and delete workspace files.
- [ ] `apply_patch` rejects outside-workspace paths.
- [ ] `apply_patch` rejects mismatched hunks without modifying the file.
- [ ] Turn-loop tests prove Phase 2 tools execute through model tool calls.
- [ ] `docs/mvp-verification.md` records Phase 2 verification evidence.
- [ ] No edited/new source file exceeds 1000 physical lines.

## Codex-Like Runtime Phase 2 Completion Evidence

Completed: 2026-06-27

Commits:
- `5c8f147` feat: add phase 2 tool registration plumbing
- `d4b3ab5` fix: gate command tool registration by runtime mode
- `792778c` docs: add codex-like runtime phase 2 plan
- `e3860ec` fix: make phase 2 skeleton tools fail explicitly
- `f4842d7` feat: add workspace search tool
- `c7e9b4b` fix: bound workspace search output
- `bd9efd6` feat: add development command tool
- `61e943d` fix: harden command execution cleanup
- `ddf2704` fix: keep truncated command results bounded
- `966ab6f` fix: tighten command deny policy
- `dc79d0c` fix: match command deny basenames
- `956ba3e` feat: add workspace patch tool
- `14c7cd7` test: update patch built-in dispatch coverage
- `d305a2b` fix: harden patch validation
- `6b0b24c` test: split patch tool tests
- `695882e` fix: validate malformed patch hunk lines
- `14898f1` test: cover phase 2 runtime tools

Focused verification:
- `pixi run cargo test -p agent-runtime tools::search::tests -- --nocapture`: PASS, 6/6
- `pixi run cargo test -p agent-runtime tools::command::tests -- --nocapture`: PASS, 13/13
- `pixi run cargo test -p agent-runtime tools::patch::patch_tests -- --nocapture`: PASS, 18/18
- `pixi run cargo test -p agent-runtime turn::tests::phase_two_search_files_executes_through_turn_loop`: PASS
- `pixi run cargo test -p agent-runtime turn::tests::phase_two_exec_command_is_advertised_only_when_allowed`: PASS
- `pixi run cargo test -p agent-runtime turn::tests::phase_two_apply_patch_executes_through_turn_loop`: PASS

Full verification:
- `pixi run cargo test --workspace`: PASS, `agent-runtime` 110/110, `agent-server` 11/11, `model-gateway` 15/15
- `pixi run cargo clippy --workspace --all-targets -- -D warnings`: PASS
- `pixi run cargo fmt --all --check`: PASS
- `git diff --check HEAD`: PASS
- Line count check: PASS, no edited/new source file exceeds 1000 physical lines; largest checked files were `crates/agent-runtime/src/tools/builtin.rs` and `crates/agent-server/src/api.rs` at 943 lines.

Review:
- Task 1 spec compliance reviewer: PASS after command registration was gated by runtime mode.
- Task 1 code quality reviewer: PASS after skeleton tools returned explicit failures.
- Task 2 spec compliance reviewer: PASS.
- Task 2 code quality reviewer: PASS after search output was bounded and `rg` streaming was added.
- Task 3 spec compliance reviewer: PASS after process cleanup, timeout, and deny-rule fixes.
- Task 3 code quality reviewer: PASS after command process groups, bounded registry results, command substitution denial, and basename matching fixes.
- Task 4 spec compliance reviewer: PASS after strict patch validation and malformed hunk handling.
- Task 4 code quality reviewer: PASS after conflict detection and patch test split.
