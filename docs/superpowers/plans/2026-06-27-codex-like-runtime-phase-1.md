# Codex-Like Runtime Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build Phase 1 of the Codex-like runtime foundation: instruction context, workspace-safe built-in filesystem tools, capability policy, structured tool results, and turn-loop integration.

**Architecture:** Keep the existing Rust runtime and model gateway, but add focused runtime modules for instructions and tools. `TurnRunner` will keep its current loop shape while delegating model input construction to `InstructionContext` and tool schema/execution to `ToolRegistry`.

**Tech Stack:** Rust 2024, Tokio, async-trait, serde, serde_json, anyhow, model-gateway, pixi, cargo test.

---

## Source Design

Use the approved design spec:

- `docs/superpowers/specs/2026-06-27-codex-like-runtime-migration-design.md`

Phase 1 scope from the spec:

- `InstructionContext` module.
- AGENTS.md discovery from workspace root to cwd.
- `ToolRuntime` or equivalent executor abstraction.
- Minimal capability policy with `read_only`, `workspace_write`, and `command_disabled`.
- Built-in `create_directory`, `list_directory`, `file_metadata`, `read_text_file`, and `write_text_file`.
- Structured tool success and failure payloads.
- Updated turn loop input construction.
- Tests proving a filesystem tool call can actually create a directory.
- Explicit completion endpoint behavior when tools are present.

## Current Baseline Notes

The working tree may already contain unrelated changes. Do not revert or reformat unrelated files.

Current runtime facts:

- `crates/agent-runtime/src/turn.rs` already has a multi-step tool loop.
- `crates/agent-runtime/src/skill.rs` already supports `skill.json` runtime tools and packaged skill loading in the current working tree.
- `skills/echo` remains the runtime skill fixture.
- `crates/model-gateway/src/responses.rs` currently drops tools for `EndpointType::Completion` by converting only the latest prompt into a completion request.

## File Structure

Create these files:

- `crates/agent-runtime/src/instructions.rs`
  - Owns instruction authority, AGENTS.md discovery, instruction block rendering, and model input construction.
- `crates/agent-runtime/src/tools/mod.rs`
  - Owns `ToolRegistry`, `RuntimeMode`, `ToolDefinition`, `ToolPermission`, and routing between built-in tools and `SkillRegistry`.
- `crates/agent-runtime/src/tools/builtin.rs`
  - Owns Phase 1 built-in filesystem tool schemas and execution.
- `crates/agent-runtime/src/tools/path.rs`
  - Owns workspace path canonicalization and path escape prevention.
- `crates/agent-runtime/src/tools/result.rs`
  - Owns structured `ToolResult`, `ToolError`, and `ToolResultMetadata` envelopes.

Modify these files:

- `crates/agent-runtime/src/lib.rs`
  - Export `instructions` and `tools`.
- `crates/agent-runtime/src/turn.rs`
  - Replace direct `SkillRegistry` execution with `ToolRegistry`.
  - Build initial model input through `InstructionContext`.
  - Keep `TurnRunner::new(model, skills)` for existing callers.
  - Add `TurnRunner::new_with_config(model, skills, config)` for tests and server wiring.
- `crates/model-gateway/src/provider.rs`
  - Add `ProviderProfile::supports_tools()`.
- `crates/model-gateway/src/responses.rs`
  - Reject non-empty tool schemas for `EndpointType::Completion`.
- `crates/agent-server/src/main.rs`
  - Build a runtime config with workspace root from env or current directory.
- `crates/agent-server/src/api.rs`
  - Pass a runtime config when model settings create a per-request runner.

Do not modify desktop UI in Phase 1.

## Task 1: Add Structured Tool Results And Workspace Path Safety

**Files:**
- Create: `crates/agent-runtime/src/tools/result.rs`
- Create: `crates/agent-runtime/src/tools/path.rs`
- Create: `crates/agent-runtime/src/tools/mod.rs`
- Modify: `crates/agent-runtime/src/lib.rs`
- Test: `crates/agent-runtime/src/tools/result.rs`
- Test: `crates/agent-runtime/src/tools/path.rs`

- [ ] **Step 1: Create the `tools` module skeleton**

Add `crates/agent-runtime/src/tools/mod.rs`:

```rust
pub mod result;

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandMode {
    Disabled,
}

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub workspace_root: PathBuf,
    pub cwd: PathBuf,
    pub mode: RuntimeMode,
    pub command_mode: CommandMode,
    pub max_tool_calls_per_turn: usize,
    pub tool_timeout_ms: u64,
    pub output_limit_bytes: usize,
}

impl RuntimeConfig {
    pub fn workspace_write(workspace_root: PathBuf) -> Self {
        Self {
            cwd: workspace_root.clone(),
            workspace_root,
            mode: RuntimeMode::WorkspaceWrite,
            command_mode: CommandMode::Disabled,
            max_tool_calls_per_turn: 16,
            tool_timeout_ms: 30_000,
            output_limit_bytes: 64 * 1024,
        }
    }

    pub fn read_only(workspace_root: PathBuf) -> Self {
        Self {
            cwd: workspace_root.clone(),
            workspace_root,
            mode: RuntimeMode::ReadOnly,
            command_mode: CommandMode::Disabled,
            max_tool_calls_per_turn: 16,
            tool_timeout_ms: 30_000,
            output_limit_bytes: 64 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeMode {
    ReadOnly,
    WorkspaceWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolPermission {
    ReadWorkspace,
    WriteWorkspace,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub permission: ToolPermission,
}

pub fn permission_allowed(mode: RuntimeMode, permission: ToolPermission) -> bool {
    match (mode, permission) {
        (RuntimeMode::ReadOnly, ToolPermission::ReadWorkspace) => true,
        (RuntimeMode::ReadOnly, ToolPermission::WriteWorkspace) => false,
        (RuntimeMode::WorkspaceWrite, ToolPermission::ReadWorkspace) => true,
        (RuntimeMode::WorkspaceWrite, ToolPermission::WriteWorkspace) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_blocks_workspace_writes() {
        assert!(permission_allowed(
            RuntimeMode::ReadOnly,
            ToolPermission::ReadWorkspace
        ));
        assert!(!permission_allowed(
            RuntimeMode::ReadOnly,
            ToolPermission::WriteWorkspace
        ));
    }

    #[test]
    fn runtime_config_defaults_to_command_disabled() {
        let config = RuntimeConfig::workspace_write(PathBuf::from("/tmp/workspace"));

        assert_eq!(config.command_mode, CommandMode::Disabled);
        assert_eq!(config.max_tool_calls_per_turn, 16);
        assert_eq!(config.tool_timeout_ms, 30_000);
    }
}
```

Modify `crates/agent-runtime/src/lib.rs`:

```rust
pub mod events;
pub mod session;
pub mod skill;
pub mod storage;
pub mod tools;
pub mod turn;
```

- [ ] **Step 2: Write failing tests for structured tool results**

Create `crates/agent-runtime/src/tools/result.rs` with the tests first:

```rust
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolResult {
    pub ok: bool,
    pub tool: String,
    pub call_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ToolError>,
    pub metadata: ToolResultMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolResultMetadata {
    pub duration_ms: u64,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub output_truncated: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_result_uses_stable_json_envelope() {
        let result = ToolResult::success(
            "create_directory",
            "call-1",
            json!({ "path": "test", "created": true }),
            ToolResultMetadata::default(),
        );

        let value = serde_json::to_value(result).unwrap();

        assert_eq!(value["ok"], true);
        assert_eq!(value["tool"], "create_directory");
        assert_eq!(value["call_id"], "call-1");
        assert_eq!(value["data"]["path"], "test");
        assert!(value.get("error").is_none());
        assert_eq!(value["metadata"]["output_truncated"], false);
    }

    #[test]
    fn failure_result_uses_stable_json_envelope() {
        let result = ToolResult::failure(
            "create_directory",
            "call-1",
            "path_outside_workspace",
            "Path must stay inside the workspace.",
            false,
            ToolResultMetadata::default(),
        );

        let value = serde_json::to_value(result).unwrap();

        assert_eq!(value["ok"], false);
        assert_eq!(value["tool"], "create_directory");
        assert_eq!(value["call_id"], "call-1");
        assert_eq!(value["error"]["code"], "path_outside_workspace");
        assert_eq!(value["error"]["retryable"], false);
        assert!(value.get("data").is_none());
    }
}
```

- [ ] **Step 3: Run the structured result tests and verify they fail**

Run:

```bash
pixi run cargo test -p agent-runtime tools::result -- --nocapture
```

Expected: FAIL with missing `ToolResult::success`, `ToolResult::failure`, or `ToolResultMetadata::default`.

- [ ] **Step 4: Implement structured result constructors**

Add these impl blocks above the test module in `crates/agent-runtime/src/tools/result.rs`:

```rust
impl ToolResult {
    pub fn success(
        tool: impl Into<String>,
        call_id: impl Into<String>,
        data: Value,
        metadata: ToolResultMetadata,
    ) -> Self {
        Self {
            ok: true,
            tool: tool.into(),
            call_id: call_id.into(),
            data: Some(data),
            error: None,
            metadata,
        }
    }

    pub fn failure(
        tool: impl Into<String>,
        call_id: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
        retryable: bool,
        metadata: ToolResultMetadata,
    ) -> Self {
        Self {
            ok: false,
            tool: tool.into(),
            call_id: call_id.into(),
            data: None,
            error: Some(ToolError {
                code: code.into(),
                message: message.into(),
                retryable,
            }),
            metadata,
        }
    }

    pub fn into_value(self) -> Value {
        serde_json::to_value(self).expect("tool result should serialize")
    }
}

impl Default for ToolResultMetadata {
    fn default() -> Self {
        Self {
            duration_ms: 0,
            stdout_truncated: false,
            stderr_truncated: false,
            output_truncated: false,
        }
    }
}
```

- [ ] **Step 5: Run the structured result tests and verify they pass**

Run:

```bash
pixi run cargo test -p agent-runtime tools::result -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Write failing tests for workspace path resolution**

Create `crates/agent-runtime/src/tools/path.rs`:

```rust
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacePath {
    pub relative: PathBuf,
    pub absolute: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_path_inside_workspace() {
        let root = temp_workspace_root("inside");
        std::fs::create_dir_all(&root).unwrap();

        let resolved = resolve_workspace_path(&root, "nested/file.txt").unwrap();

        assert_eq!(resolved.relative, PathBuf::from("nested/file.txt"));
        assert!(resolved.absolute.starts_with(&root));
        remove_workspace(root);
    }

    #[test]
    fn rejects_parent_traversal() {
        let root = temp_workspace_root("parent-traversal");
        std::fs::create_dir_all(&root).unwrap();

        let error = resolve_workspace_path(&root, "../escape.txt").unwrap_err();

        assert!(error.to_string().contains("path must stay inside workspace"));
        remove_workspace(root);
    }

    #[test]
    fn rejects_absolute_path_outside_workspace() {
        let root = temp_workspace_root("absolute-outside");
        std::fs::create_dir_all(&root).unwrap();
        let outside = std::env::temp_dir().join("generalagent-outside.txt");

        let error = resolve_workspace_path(&root, outside.to_string_lossy().as_ref()).unwrap_err();

        assert!(error.to_string().contains("path must stay inside workspace"));
        remove_workspace(root);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_existing_symlink_that_escapes_workspace() {
        let root = temp_workspace_root("symlink-escape");
        let outside = temp_workspace_root("symlink-outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "secret").unwrap();
        std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("link.txt")).unwrap();
        let resolved = resolve_workspace_path(&root, "link.txt").unwrap();

        let error = ensure_existing_path_inside_workspace(&root, &resolved.absolute).unwrap_err();

        assert!(error.to_string().contains("path must stay inside workspace"));
        remove_workspace(root);
        remove_workspace(outside);
    }

    fn temp_workspace_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "generalagent-path-{name}-{}",
            uuid::Uuid::new_v4()
        ))
    }

    fn remove_workspace(root: PathBuf) {
        if root.exists() {
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
```

Modify the top of `crates/agent-runtime/src/tools/mod.rs`:

```rust
pub mod path;
pub mod result;
```

- [ ] **Step 7: Run the path tests and verify they fail**

Run:

```bash
pixi run cargo test -p agent-runtime tools::path -- --nocapture
```

Expected: FAIL with missing `resolve_workspace_path`.

- [ ] **Step 8: Implement workspace path resolution**

Add this implementation above the tests in `crates/agent-runtime/src/tools/path.rs`:

```rust
pub fn resolve_workspace_path(root: &Path, requested: &str) -> anyhow::Result<WorkspacePath> {
    if requested.trim().is_empty() {
        anyhow::bail!("path must not be empty");
    }

    let canonical_root = std::fs::canonicalize(root)
        .map_err(|error| anyhow::anyhow!("failed to resolve workspace root: {error}"))?;
    let requested_path = Path::new(requested);

    let relative = if requested_path.is_absolute() {
        let canonical_requested = std::fs::canonicalize(requested_path)
            .unwrap_or_else(|_| requested_path.to_path_buf());
        if !canonical_requested.starts_with(&canonical_root) {
            anyhow::bail!("path must stay inside workspace");
        }
        canonical_requested
            .strip_prefix(&canonical_root)
            .map_err(|_| anyhow::anyhow!("path must stay inside workspace"))?
            .to_path_buf()
    } else {
        normalize_relative_path(requested_path)?
    };

    let absolute = canonical_root.join(&relative);
    if !absolute.starts_with(&canonical_root) {
        anyhow::bail!("path must stay inside workspace");
    }

    Ok(WorkspacePath { relative, absolute })
}

pub fn ensure_existing_path_inside_workspace(root: &Path, absolute: &Path) -> anyhow::Result<()> {
    let canonical_root = std::fs::canonicalize(root)
        .map_err(|error| anyhow::anyhow!("failed to resolve workspace root: {error}"))?;
    let canonical_path = std::fs::canonicalize(absolute)
        .map_err(|error| anyhow::anyhow!("failed to resolve workspace path: {error}"))?;

    if !canonical_path.starts_with(&canonical_root) {
        anyhow::bail!("path must stay inside workspace");
    }

    Ok(())
}

fn normalize_relative_path(path: &Path) -> anyhow::Result<PathBuf> {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir => anyhow::bail!("path must stay inside workspace"),
            Component::RootDir | Component::Prefix(_) => {
                anyhow::bail!("path must stay inside workspace")
            }
        }
    }

    if normalized.as_os_str().is_empty() {
        return Ok(PathBuf::from("."));
    }

    Ok(normalized)
}
```

- [ ] **Step 9: Run Task 1 tests**

Run:

```bash
pixi run cargo test -p agent-runtime tools:: -- --nocapture
```

Expected: PASS for result and path tests.

- [ ] **Step 10: Commit Task 1**

```bash
git add crates/agent-runtime/src/lib.rs crates/agent-runtime/src/tools
git commit -m "feat: add runtime tool result and path contracts"
```

## Task 2: Add Built-In Workspace Filesystem Tools

**Files:**
- Modify: `crates/agent-runtime/src/tools/builtin.rs`
- Modify: `crates/agent-runtime/src/tools/mod.rs`
- Test: `crates/agent-runtime/src/tools/builtin.rs`

Execution granularity:

- Add the test module first and watch it fail.
- Implement `BuiltInTools` struct and tool definitions.
- Implement `create_directory`, run the built-in tests, and confirm only later tool tests still fail.
- Implement `write_text_file` and `read_text_file`, run the built-in tests again.
- Implement `list_directory` and `file_metadata`, run the built-in tests again.
- Add structured error wrapping and symlink tests, then run the full `tools::` suite.

- [ ] **Step 1: Write failing tests for built-in filesystem tools**

Create `crates/agent-runtime/src/tools/builtin.rs` with tests first:

```rust
use super::{
    RuntimeMode, ToolDefinition, ToolPermission, permission_allowed,
    path::{ensure_existing_path_inside_workspace, resolve_workspace_path},
    result::{ToolResult, ToolResultMetadata},
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Instant;

pub const CREATE_DIRECTORY: &str = "create_directory";
pub const LIST_DIRECTORY: &str = "list_directory";
pub const FILE_METADATA: &str = "file_metadata";
pub const READ_TEXT_FILE: &str = "read_text_file";
pub const WRITE_TEXT_FILE: &str = "write_text_file";

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_directory_creates_workspace_directory() {
        let root = temp_workspace_root("create-directory");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let tools = BuiltInTools::new(root.clone(), RuntimeMode::WorkspaceWrite);

        let result = tools
            .execute(CREATE_DIRECTORY, "call-1", json!({ "path": "test" }))
            .await
            .unwrap();

        assert_eq!(result["ok"], true);
        assert_eq!(result["data"]["path"], "test");
        assert_eq!(result["data"]["created"], true);
        assert!(root.join("test").is_dir());
        remove_workspace(root).await;
    }

    #[tokio::test]
    async fn read_only_mode_blocks_create_directory() {
        let root = temp_workspace_root("readonly-create");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let tools = BuiltInTools::new(root.clone(), RuntimeMode::ReadOnly);

        let result = tools
            .execute(CREATE_DIRECTORY, "call-1", json!({ "path": "test" }))
            .await
            .unwrap();

        assert_eq!(result["ok"], false);
        assert_eq!(result["error"]["code"], "permission_denied");
        assert!(!root.join("test").exists());
        remove_workspace(root).await;
    }

    #[tokio::test]
    async fn write_and_read_text_file_round_trip() {
        let root = temp_workspace_root("write-read");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let tools = BuiltInTools::new(root.clone(), RuntimeMode::WorkspaceWrite);

        let write = tools
            .execute(
                WRITE_TEXT_FILE,
                "call-write",
                json!({
                    "path": "notes/hello.txt",
                    "content": "hello",
                    "create_parent_dirs": true,
                    "overwrite": false
                }),
            )
            .await
            .unwrap();
        let read = tools
            .execute(READ_TEXT_FILE, "call-read", json!({ "path": "notes/hello.txt" }))
            .await
            .unwrap();

        assert_eq!(write["ok"], true);
        assert_eq!(read["ok"], true);
        assert_eq!(read["data"]["content"], "hello");
        remove_workspace(root).await;
    }

    #[tokio::test]
    async fn list_directory_returns_deterministic_entries() {
        let root = temp_workspace_root("list-directory");
        tokio::fs::create_dir_all(root.join("folder")).await.unwrap();
        tokio::fs::write(root.join("b.txt"), "b").await.unwrap();
        tokio::fs::write(root.join("a.txt"), "a").await.unwrap();
        let tools = BuiltInTools::new(root.clone(), RuntimeMode::ReadOnly);

        let result = tools
            .execute(LIST_DIRECTORY, "call-1", json!({ "path": "." }))
            .await
            .unwrap();

        assert_eq!(result["ok"], true);
        assert_eq!(result["data"]["entries"][0]["name"], "a.txt");
        assert_eq!(result["data"]["entries"][1]["name"], "b.txt");
        assert_eq!(result["data"]["entries"][2]["name"], "folder");
        remove_workspace(root).await;
    }

    #[tokio::test]
    async fn file_metadata_reports_missing_path() {
        let root = temp_workspace_root("metadata-missing");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let tools = BuiltInTools::new(root.clone(), RuntimeMode::ReadOnly);

        let result = tools
            .execute(FILE_METADATA, "call-1", json!({ "path": "missing.txt" }))
            .await
            .unwrap();

        assert_eq!(result["ok"], true);
        assert_eq!(result["data"]["exists"], false);
        remove_workspace(root).await;
    }

    #[tokio::test]
    async fn list_directory_applies_entry_limit() {
        let root = temp_workspace_root("list-limit");
        tokio::fs::create_dir_all(&root).await.unwrap();
        for index in 0..205 {
            tokio::fs::write(root.join(format!("{index:03}.txt")), "x")
                .await
                .unwrap();
        }
        let tools = BuiltInTools::new(root.clone(), RuntimeMode::ReadOnly);

        let result = tools
            .execute(LIST_DIRECTORY, "call-1", json!({ "path": "." }))
            .await
            .unwrap();

        assert_eq!(result["ok"], true);
        assert_eq!(result["data"]["entries"].as_array().unwrap().len(), 200);
        assert_eq!(result["data"]["truncated"], true);
        remove_workspace(root).await;
    }

    fn temp_workspace_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "generalagent-builtin-{name}-{}",
            uuid::Uuid::new_v4()
        ))
    }

    async fn remove_workspace(root: PathBuf) {
        if root.exists() {
            tokio::fs::remove_dir_all(root).await.unwrap();
        }
    }
}
```

Modify the top of `crates/agent-runtime/src/tools/mod.rs`:

```rust
pub mod builtin;
pub mod path;
pub mod result;
```

- [ ] **Step 2: Run the built-in tool tests and verify they fail**

Run:

```bash
pixi run cargo test -p agent-runtime tools::builtin -- --nocapture
```

Expected: FAIL with missing `BuiltInTools`.

- [ ] **Step 3: Implement built-in tool definitions and dispatch**

Add this implementation above the tests in `crates/agent-runtime/src/tools/builtin.rs`:

```rust
#[derive(Debug, Clone)]
pub struct BuiltInTools {
    workspace_root: PathBuf,
    mode: RuntimeMode,
    read_limit_bytes: usize,
    list_limit_entries: usize,
}

impl BuiltInTools {
    pub fn new(workspace_root: PathBuf, mode: RuntimeMode) -> Self {
        Self {
            workspace_root,
            mode,
            read_limit_bytes: 64 * 1024,
            list_limit_entries: 200,
        }
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        vec![
            tool_definition(
                CREATE_DIRECTORY,
                "Create a directory inside the workspace.",
                ToolPermission::WriteWorkspace,
                json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" }
                    },
                    "required": ["path"]
                }),
            ),
            tool_definition(
                LIST_DIRECTORY,
                "List entries in a workspace directory.",
                ToolPermission::ReadWorkspace,
                json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" }
                    },
                    "required": ["path"]
                }),
            ),
            tool_definition(
                FILE_METADATA,
                "Return metadata for a workspace path.",
                ToolPermission::ReadWorkspace,
                json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" }
                    },
                    "required": ["path"]
                }),
            ),
            tool_definition(
                READ_TEXT_FILE,
                "Read a UTF-8 text file inside the workspace.",
                ToolPermission::ReadWorkspace,
                json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" }
                    },
                    "required": ["path"]
                }),
            ),
            tool_definition(
                WRITE_TEXT_FILE,
                "Write a UTF-8 text file inside the workspace.",
                ToolPermission::WriteWorkspace,
                json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "content": { "type": "string" },
                        "create_parent_dirs": { "type": "boolean" },
                        "overwrite": { "type": "boolean" }
                    },
                    "required": ["path", "content"]
                }),
            ),
        ]
    }

    pub async fn execute(
        &self,
        name: &str,
        call_id: &str,
        arguments: Value,
    ) -> anyhow::Result<Value> {
        let started = Instant::now();
        let Some(definition) = self.definitions().into_iter().find(|tool| tool.name == name) else {
            return Ok(ToolResult::failure(
                name,
                call_id,
                "unknown_tool",
                format!("Unknown built-in tool: {name}"),
                false,
                metadata(started),
            )
            .into_value());
        };

        if !permission_allowed(self.mode, definition.permission) {
            return Ok(ToolResult::failure(
                name,
                call_id,
                "permission_denied",
                "Tool is not allowed in the current runtime mode.",
                false,
                metadata(started),
            )
            .into_value());
        }

        let result = match name {
            CREATE_DIRECTORY => self.create_directory(call_id, arguments, started).await,
            LIST_DIRECTORY => self.list_directory(call_id, arguments, started).await,
            FILE_METADATA => self.file_metadata(call_id, arguments, started).await,
            READ_TEXT_FILE => self.read_text_file(call_id, arguments, started).await,
            WRITE_TEXT_FILE => self.write_text_file(call_id, arguments, started).await,
            _ => Ok(ToolResult::failure(
                name,
                call_id,
                "unknown_tool",
                format!("Unknown built-in tool: {name}"),
                false,
                metadata(started),
            )
            .into_value()),
        }?;

        Ok(result)
    }

    async fn create_directory(
        &self,
        call_id: &str,
        arguments: Value,
        started: Instant,
    ) -> anyhow::Result<Value> {
        let path = required_string(&arguments, "path")?;
        let resolved = resolve_workspace_path(&self.workspace_root, path)?;
        if resolved.absolute.exists() {
            ensure_existing_path_inside_workspace(&self.workspace_root, &resolved.absolute)?;
        }
        let existed = resolved.absolute.is_dir();
        tokio::fs::create_dir_all(&resolved.absolute).await?;

        Ok(ToolResult::success(
            CREATE_DIRECTORY,
            call_id,
            json!({
                "path": resolved.relative.to_string_lossy(),
                "absolute_path": resolved.absolute.to_string_lossy(),
                "created": !existed
            }),
            metadata(started),
        )
        .into_value())
    }

    async fn list_directory(
        &self,
        call_id: &str,
        arguments: Value,
        started: Instant,
    ) -> anyhow::Result<Value> {
        let path = required_string(&arguments, "path")?;
        let resolved = resolve_workspace_path(&self.workspace_root, path)?;
        ensure_existing_path_inside_workspace(&self.workspace_root, &resolved.absolute)?;
        let mut entries = tokio::fs::read_dir(&resolved.absolute).await?;
        let mut rows = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            if rows.len() >= self.list_limit_entries {
                break;
            }
            let metadata = entry.metadata().await?;
            let file_type = if metadata.is_dir() {
                "directory"
            } else if metadata.is_file() {
                "file"
            } else {
                "other"
            };
            let name = entry.file_name().to_string_lossy().to_string();
            let relative_path = resolved.relative.join(&name);
            rows.push(json!({
                "name": name,
                "path": relative_path.to_string_lossy(),
                "type": file_type,
                "size": metadata.len()
            }));
        }

        rows.sort_by(|left, right| {
            left["name"]
                .as_str()
                .unwrap_or_default()
                .cmp(right["name"].as_str().unwrap_or_default())
        });

        Ok(ToolResult::success(
            LIST_DIRECTORY,
            call_id,
            json!({
                "path": resolved.relative.to_string_lossy(),
                "entries": rows,
                "truncated": rows.len() >= self.list_limit_entries
            }),
            metadata(started),
        )
        .into_value())
    }

    async fn file_metadata(
        &self,
        call_id: &str,
        arguments: Value,
        started: Instant,
    ) -> anyhow::Result<Value> {
        let path = required_string(&arguments, "path")?;
        let resolved = resolve_workspace_path(&self.workspace_root, path)?;
        let metadata = match tokio::fs::symlink_metadata(&resolved.absolute).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ToolResult::success(
                    FILE_METADATA,
                    call_id,
                    json!({ "path": resolved.relative.to_string_lossy(), "exists": false }),
                    metadata(started),
                )
                .into_value());
            }
            Err(error) => return Err(error.into()),
        };
        let file_type = if metadata.is_dir() {
            "directory"
        } else if metadata.is_file() {
            "file"
        } else {
            "other"
        };

        Ok(ToolResult::success(
            FILE_METADATA,
            call_id,
            json!({
                "path": resolved.relative.to_string_lossy(),
                "exists": true,
                "type": file_type,
                "size": metadata.len()
            }),
            metadata(started),
        )
        .into_value())
    }

    async fn read_text_file(
        &self,
        call_id: &str,
        arguments: Value,
        started: Instant,
    ) -> anyhow::Result<Value> {
        let path = required_string(&arguments, "path")?;
        let resolved = resolve_workspace_path(&self.workspace_root, path)?;
        ensure_existing_path_inside_workspace(&self.workspace_root, &resolved.absolute)?;
        let bytes = tokio::fs::read(&resolved.absolute).await?;
        let truncated = bytes.len() > self.read_limit_bytes;
        let visible = if truncated {
            &bytes[..self.read_limit_bytes]
        } else {
            &bytes
        };
        let content = String::from_utf8_lossy(visible).to_string();

        Ok(ToolResult::success(
            READ_TEXT_FILE,
            call_id,
            json!({
                "path": resolved.relative.to_string_lossy(),
                "content": content,
                "truncated": truncated
            }),
            ToolResultMetadata {
                output_truncated: truncated,
                ..metadata(started)
            },
        )
        .into_value())
    }

    async fn write_text_file(
        &self,
        call_id: &str,
        arguments: Value,
        started: Instant,
    ) -> anyhow::Result<Value> {
        let path = required_string(&arguments, "path")?;
        let content = required_string(&arguments, "content")?;
        let create_parent_dirs = optional_bool(&arguments, "create_parent_dirs");
        let overwrite = optional_bool(&arguments, "overwrite");
        let resolved = resolve_workspace_path(&self.workspace_root, path)?;

        if resolved.absolute.exists() && !overwrite {
            return Ok(ToolResult::failure(
                WRITE_TEXT_FILE,
                call_id,
                "path_exists",
                "Refusing to overwrite existing file without overwrite=true.",
                false,
                metadata(started),
            )
            .into_value());
        }
        if resolved.absolute.exists() {
            ensure_existing_path_inside_workspace(&self.workspace_root, &resolved.absolute)?;
        }
        if create_parent_dirs {
            if let Some(parent) = resolved.absolute.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
        }

        tokio::fs::write(&resolved.absolute, content).await?;

        Ok(ToolResult::success(
            WRITE_TEXT_FILE,
            call_id,
            json!({
                "path": resolved.relative.to_string_lossy(),
                "bytes": content.as_bytes().len()
            }),
            metadata(started),
        )
        .into_value())
    }
}

fn tool_definition(
    name: &str,
    description: &str,
    permission: ToolPermission,
    input_schema: Value,
) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: description.to_string(),
        input_schema,
        permission,
    }
}

fn required_string<'a>(arguments: &'a Value, key: &str) -> anyhow::Result<&'a str> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing required string argument: {key}"))
}

fn optional_bool(arguments: &Value, key: &str) -> bool {
    arguments.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn metadata(started: Instant) -> ToolResultMetadata {
    ToolResultMetadata {
        duration_ms: started.elapsed().as_millis() as u64,
        ..ToolResultMetadata::default()
    }
}
```

- [ ] **Step 4: Add structured failure tests for built-in tools**

Add these tests inside `mod tests` in `crates/agent-runtime/src/tools/builtin.rs`:

```rust
#[tokio::test]
async fn path_escape_returns_structured_failure() {
    let root = temp_workspace_root("path-escape");
    tokio::fs::create_dir_all(&root).await.unwrap();
    let tools = BuiltInTools::new(root.clone(), RuntimeMode::WorkspaceWrite);

    let result = tools
        .execute(CREATE_DIRECTORY, "call-1", json!({ "path": "../escape" }))
        .await
        .unwrap();

    assert_eq!(result["ok"], false);
    assert_eq!(result["error"]["code"], "path_outside_workspace");
    assert!(!root.join("escape").exists());
    remove_workspace(root).await;
}

#[tokio::test]
async fn missing_read_returns_structured_failure() {
    let root = temp_workspace_root("missing-read");
    tokio::fs::create_dir_all(&root).await.unwrap();
    let tools = BuiltInTools::new(root.clone(), RuntimeMode::ReadOnly);

    let result = tools
        .execute(READ_TEXT_FILE, "call-1", json!({ "path": "missing.txt" }))
        .await
        .unwrap();

    assert_eq!(result["ok"], false);
    assert_eq!(result["error"]["code"], "path_not_found");
    remove_workspace(root).await;
}

#[tokio::test]
async fn invalid_arguments_return_structured_failure() {
    let root = temp_workspace_root("invalid-arguments");
    tokio::fs::create_dir_all(&root).await.unwrap();
    let tools = BuiltInTools::new(root.clone(), RuntimeMode::ReadOnly);

    let result = tools
        .execute(READ_TEXT_FILE, "call-1", json!({ "path": "" }))
        .await
        .unwrap();

    assert_eq!(result["ok"], false);
    assert_eq!(result["error"]["code"], "invalid_arguments");
    remove_workspace(root).await;
}

#[cfg(unix)]
#[tokio::test]
async fn reading_symlink_to_outside_workspace_returns_structured_failure() {
    let root = temp_workspace_root("symlink-read");
    let outside = temp_workspace_root("symlink-read-outside");
    tokio::fs::create_dir_all(&root).await.unwrap();
    tokio::fs::create_dir_all(&outside).await.unwrap();
    tokio::fs::write(outside.join("secret.txt"), "secret")
        .await
        .unwrap();
    std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("link.txt")).unwrap();
    let tools = BuiltInTools::new(root.clone(), RuntimeMode::ReadOnly);

    let result = tools
        .execute(READ_TEXT_FILE, "call-1", json!({ "path": "link.txt" }))
        .await
        .unwrap();

    assert_eq!(result["ok"], false);
    assert_eq!(result["error"]["code"], "path_outside_workspace");
    remove_workspace(root).await;
    remove_workspace(outside).await;
}
```

- [ ] **Step 5: Wrap built-in tool failures in stable tool result errors**

In `BuiltInTools::execute`, replace the branch call and return block:

```rust
let result = match name {
    CREATE_DIRECTORY => self.create_directory(call_id, arguments, started).await,
    LIST_DIRECTORY => self.list_directory(call_id, arguments, started).await,
    FILE_METADATA => self.file_metadata(call_id, arguments, started).await,
    READ_TEXT_FILE => self.read_text_file(call_id, arguments, started).await,
    WRITE_TEXT_FILE => self.write_text_file(call_id, arguments, started).await,
    _ => Ok(ToolResult::failure(
        name,
        call_id,
        "unknown_tool",
        format!("Unknown built-in tool: {name}"),
        false,
        metadata(started),
    )
    .into_value()),
};

Ok(match result {
    Ok(value) => value,
    Err(error) => structured_builtin_error(name, call_id, error, started),
})
```

Add these helpers below `optional_bool`:

```rust
fn structured_builtin_error(
    tool: &str,
    call_id: &str,
    error: anyhow::Error,
    started: Instant,
) -> Value {
    let message = error.to_string();
    let code = if message.contains("path must stay inside workspace") {
        "path_outside_workspace"
    } else if message.contains("missing required string argument") || message.contains("path must not be empty") {
        "invalid_arguments"
    } else if message.contains("No such file") || message.contains("not found") {
        "path_not_found"
    } else {
        "internal_error"
    };

    ToolResult::failure(tool, call_id, code, message, false, metadata(started)).into_value()
}
```

Run:

```bash
pixi run cargo test -p agent-runtime tools::builtin -- --nocapture
```

Expected: PASS for the built-in tool tests.

- [ ] **Step 6: Add ToolRegistry routing over built-ins and runtime skills**

Append this code to `crates/agent-runtime/src/tools/mod.rs` after `permission_allowed`:

```rust
use crate::skill::SkillRegistry;
use builtin::BuiltInTools;
use result::{ToolResult, ToolResultMetadata};
use serde_json::Value;
use tokio::time::{Duration, timeout};

pub struct ToolRegistry {
    builtins: BuiltInTools,
    skills: SkillRegistry,
    tool_timeout: Duration,
    output_limit_bytes: usize,
}

impl ToolRegistry {
    pub fn new(skills: SkillRegistry, config: &RuntimeConfig) -> Self {
        Self {
            builtins: BuiltInTools::new(config.workspace_root.clone(), config.mode),
            skills,
            tool_timeout: Duration::from_millis(config.tool_timeout_ms),
            output_limit_bytes: config.output_limit_bytes,
        }
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let mut definitions = self.builtins.definitions();
        definitions.extend(self.skills.tools().into_iter().map(|tool| ToolDefinition {
            name: tool.name,
            description: tool.description,
            input_schema: tool.input_schema,
            permission: ToolPermission::ReadWorkspace,
        }));
        definitions
    }

    pub async fn execute(
        &self,
        name: &str,
        call_id: &str,
        arguments: Value,
    ) -> anyhow::Result<Value> {
        match timeout(
            self.tool_timeout,
            self.execute_without_timeout(name, call_id, arguments),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Ok(ToolResult::failure(
                name,
                call_id,
                "timeout",
                "Tool execution exceeded the configured timeout.",
                true,
                ToolResultMetadata::default(),
            )
            .into_value()),
        }
    }

    async fn execute_without_timeout(
        &self,
        name: &str,
        call_id: &str,
        arguments: Value,
    ) -> anyhow::Result<Value> {
        if self
            .builtins
            .definitions()
            .iter()
            .any(|definition| definition.name == name)
        {
            return self.builtins.execute(name, call_id, arguments).await;
        }

        match self.skills.execute(name, arguments).await {
            Ok(value) => Ok(self.skill_success(name, call_id, value)),
            Err(error) => Ok(ToolResult::failure(
                name,
                call_id,
                "internal_error",
                error.to_string(),
                false,
                ToolResultMetadata::default(),
            )
            .into_value()),
        }
    }

    fn skill_success(&self, name: &str, call_id: &str, value: Value) -> Value {
        if value.to_string().len() > self.output_limit_bytes {
            return ToolResult::failure(
                name,
                call_id,
                "output_limit_exceeded",
                "Tool output exceeded the configured output limit.",
                false,
                ToolResultMetadata {
                    output_truncated: true,
                    ..ToolResultMetadata::default()
                },
            )
            .into_value();
        }

        ToolResult::success(name, call_id, value, ToolResultMetadata::default()).into_value()
    }
}
```

- [ ] **Step 7: Add ToolRegistry timeout and output-limit tests**

Append these tests to `crates/agent-runtime/src/tools/mod.rs`:

```rust
#[cfg(test)]
mod registry_tests {
    use super::*;
    use serde_json::json;
    use std::path::{Path, PathBuf};

    #[tokio::test]
    async fn runtime_skill_output_limit_returns_structured_failure() {
        let root = unique_test_dir("skill-output-limit");
        write_node_skill(
            &root,
            "big",
            "big_output",
            "process.stdin.resume(); process.stdin.on('end', () => process.stdout.write(JSON.stringify({ text: 'abcdef' })));",
        )
        .await;
        let skills = SkillRegistry::load_development(&root).await.unwrap();
        let config = RuntimeConfig {
            workspace_root: root.clone(),
            cwd: root.clone(),
            mode: RuntimeMode::WorkspaceWrite,
            command_mode: CommandMode::Disabled,
            max_tool_calls_per_turn: 16,
            tool_timeout_ms: 30_000,
            output_limit_bytes: 4,
        };
        let registry = ToolRegistry::new(skills, &config);

        let result = registry
            .execute("big_output", "call-1", json!({}))
            .await
            .unwrap();

        assert_eq!(result["ok"], false);
        assert_eq!(result["error"]["code"], "output_limit_exceeded");
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn runtime_skill_timeout_returns_structured_failure() {
        let root = unique_test_dir("skill-timeout");
        write_node_skill(
            &root,
            "slow",
            "slow_output",
            "process.stdin.resume(); process.stdin.on('end', () => setTimeout(() => process.stdout.write(JSON.stringify({ text: 'done' })), 100));",
        )
        .await;
        let skills = SkillRegistry::load_development(&root).await.unwrap();
        let config = RuntimeConfig {
            workspace_root: root.clone(),
            cwd: root.clone(),
            mode: RuntimeMode::WorkspaceWrite,
            command_mode: CommandMode::Disabled,
            max_tool_calls_per_turn: 16,
            tool_timeout_ms: 5,
            output_limit_bytes: 64 * 1024,
        };
        let registry = ToolRegistry::new(skills, &config);

        let result = registry
            .execute("slow_output", "call-1", json!({}))
            .await
            .unwrap();

        assert_eq!(result["ok"], false);
        assert_eq!(result["error"]["code"], "timeout");
        remove_test_dir(root).await;
    }

    async fn write_node_skill(root: &Path, folder: &str, tool_name: &str, script: &str) {
        let skill_dir = root.join(folder);
        tokio::fs::create_dir_all(&skill_dir).await.unwrap();
        tokio::fs::write(
            skill_dir.join("skill.json"),
            json!({
                "name": folder,
                "description": "Test skill.",
                "version": "0.1.0",
                "entry": {
                    "type": "command",
                    "command": "node",
                    "args": ["index.js"]
                },
                "tools": [
                    {
                        "name": tool_name,
                        "description": "Test tool.",
                        "input_schema": { "type": "object" }
                    }
                ]
            })
            .to_string(),
        )
        .await
        .unwrap();
        tokio::fs::write(skill_dir.join("index.js"), script)
            .await
            .unwrap();
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "generalagent-tool-registry-{name}-{}",
            uuid::Uuid::new_v4()
        ))
    }

    async fn remove_test_dir(path: PathBuf) {
        if path.exists() {
            tokio::fs::remove_dir_all(path).await.unwrap();
        }
    }
}
```

- [ ] **Step 8: Run built-in and registry tests**

Run:

```bash
pixi run cargo test -p agent-runtime tools:: -- --nocapture
```

Expected: PASS for `tools::builtin`, `tools::path`, and `tools::result`.

- [ ] **Step 9: Commit Task 2**

```bash
git add crates/agent-runtime/src/tools
git commit -m "feat: add workspace filesystem tools"
```

## Task 3: Add Instruction Context And AGENTS.md Discovery

**Files:**
- Create: `crates/agent-runtime/src/instructions.rs`
- Test: `crates/agent-runtime/src/instructions.rs`

- [ ] **Step 1: Write failing instruction context tests**

Create `crates/agent-runtime/src/instructions.rs`:

```rust
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct InstructionConfig {
    pub workspace_root: PathBuf,
    pub cwd: PathBuf,
    pub max_instruction_bytes: usize,
    pub base_instructions: String,
    pub developer_instructions: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionDocument {
    pub path: PathBuf,
    pub content: String,
    pub truncated: bool,
}

pub struct InstructionContext {
    config: InstructionConfig,
    documents: Vec<InstructionDocument>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn discovers_agents_files_from_workspace_to_cwd() {
        let root = temp_workspace_root("agents-order");
        let nested = root.join("app").join("feature");
        tokio::fs::create_dir_all(&nested).await.unwrap();
        tokio::fs::write(root.join("AGENTS.md"), "root instructions")
            .await
            .unwrap();
        tokio::fs::write(root.join("app").join("AGENTS.md"), "app instructions")
            .await
            .unwrap();

        let context = InstructionContext::load(InstructionConfig::new(root.clone(), nested))
            .await
            .unwrap();

        let sources: Vec<_> = context
            .documents()
            .iter()
            .map(|document| document.path.strip_prefix(&root).unwrap().to_path_buf())
            .collect();
        assert_eq!(sources, vec![PathBuf::from("AGENTS.md"), PathBuf::from("app/AGENTS.md")]);
        remove_workspace(root).await;
    }

    #[tokio::test]
    async fn renders_model_input_before_user_message() {
        let root = temp_workspace_root("model-input");
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(root.join("AGENTS.md"), "Use tools for filesystem changes.")
            .await
            .unwrap();

        let context = InstructionContext::load(InstructionConfig::new(root.clone(), root.clone()))
            .await
            .unwrap();
        let input = context.model_input("create a test folder");

        assert_eq!(input[0]["role"], "system");
        assert!(input[0]["content"].as_str().unwrap().contains("GeneralAgent"));
        assert_eq!(input[1]["role"], "developer");
        assert!(input[1]["content"].as_str().unwrap().contains("Use tools for filesystem changes."));
        assert_eq!(input.last().unwrap()["role"], "user");
        assert_eq!(input.last().unwrap()["content"], "create a test folder");
        remove_workspace(root).await;
    }

    #[tokio::test]
    async fn truncates_large_agents_file_with_metadata() {
        let root = temp_workspace_root("truncate-agents");
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(root.join("AGENTS.md"), "abcdef").await.unwrap();
        let mut config = InstructionConfig::new(root.clone(), root.clone());
        config.max_instruction_bytes = 3;

        let context = InstructionContext::load(config).await.unwrap();
        let input = context.model_input("hello");
        let developer = input[1]["content"].as_str().unwrap();

        assert!(developer.contains("truncated=\"true\""));
        assert!(developer.contains("abc"));
        assert!(!developer.contains("abcdef"));
        remove_workspace(root).await;
    }

    fn temp_workspace_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "generalagent-instructions-{name}-{}",
            uuid::Uuid::new_v4()
        ))
    }

    async fn remove_workspace(root: PathBuf) {
        if root.exists() {
            tokio::fs::remove_dir_all(root).await.unwrap();
        }
    }
}
```

Modify `crates/agent-runtime/src/lib.rs`:

```rust
pub mod events;
pub mod instructions;
pub mod session;
pub mod skill;
pub mod storage;
pub mod tools;
pub mod turn;
```

- [ ] **Step 2: Run instruction tests and verify they fail**

Run:

```bash
pixi run cargo test -p agent-runtime instructions:: -- --nocapture
```

Expected: FAIL with missing `InstructionConfig::new`, `InstructionContext::load`, `documents`, or `model_input`.

- [ ] **Step 3: Implement instruction config and AGENTS.md discovery**

Add this implementation above the tests in `crates/agent-runtime/src/instructions.rs`:

```rust
impl InstructionConfig {
    pub fn new(workspace_root: PathBuf, cwd: PathBuf) -> Self {
        Self {
            workspace_root,
            cwd,
            max_instruction_bytes: 64 * 1024,
            base_instructions: default_base_instructions(),
            developer_instructions: None,
        }
    }
}

impl InstructionContext {
    pub async fn load(config: InstructionConfig) -> anyhow::Result<Self> {
        let documents = discover_agents_documents(&config).await?;
        Ok(Self { config, documents })
    }

    pub fn documents(&self) -> &[InstructionDocument] {
        &self.documents
    }

    pub fn model_input(&self, user_text: &str) -> Vec<Value> {
        let mut input = Vec::new();
        input.push(json!({
            "role": "system",
            "content": self.config.base_instructions
        }));

        let developer = self.render_developer_context();
        if !developer.trim().is_empty() {
            input.push(json!({
                "role": "developer",
                "content": developer
            }));
        }

        input.push(json!({
            "role": "user",
            "content": user_text
        }));
        input
    }

    fn render_developer_context(&self) -> String {
        let mut blocks = Vec::new();
        blocks.push(
            "Use available tools for concrete filesystem work. Do not answer with shell instructions when an enabled tool can perform the requested action."
                .to_string(),
        );

        if let Some(developer) = &self.config.developer_instructions {
            blocks.push(format!(
                "<developer_instructions>\n{}\n</developer_instructions>",
                developer
            ));
        }

        for document in &self.documents {
            blocks.push(format!(
                "<project_instructions source=\"{}\" bytes=\"{}\" truncated=\"{}\">\n{}\n</project_instructions>",
                document.path.display(),
                document.content.len(),
                document.truncated,
                document.content
            ));
        }

        blocks.join("\n\n")
    }
}

async fn discover_agents_documents(
    config: &InstructionConfig,
) -> anyhow::Result<Vec<InstructionDocument>> {
    let root = std::fs::canonicalize(&config.workspace_root)?;
    let cwd = std::fs::canonicalize(&config.cwd)?;
    if !cwd.starts_with(&root) {
        anyhow::bail!("cwd must stay inside workspace root");
    }

    let relative_cwd = cwd.strip_prefix(&root)?;
    let mut directories = vec![root.clone()];
    let mut current = root.clone();
    for component in relative_cwd.components() {
        current.push(component.as_os_str());
        directories.push(current.clone());
    }

    let mut documents = Vec::new();
    for directory in directories {
        let path = directory.join("AGENTS.md");
        if path.is_file() {
            documents.push(read_instruction_document(&root, &path, config.max_instruction_bytes).await?);
        }
    }

    Ok(documents)
}

async fn read_instruction_document(
    root: &Path,
    path: &Path,
    max_bytes: usize,
) -> anyhow::Result<InstructionDocument> {
    let bytes = tokio::fs::read(path).await?;
    let truncated = bytes.len() > max_bytes;
    let visible = if truncated {
        &bytes[..max_bytes]
    } else {
        &bytes
    };
    let content = String::from_utf8_lossy(visible).to_string();
    let relative = path.strip_prefix(root).unwrap_or(path).to_path_buf();

    Ok(InstructionDocument {
        path: relative,
        content,
        truncated,
    })
}

fn default_base_instructions() -> String {
    [
        "You are GeneralAgent, a Codex-like agent runtime embedded in a developer application.",
        "Follow higher-authority instructions first.",
        "Use available tools to perform concrete workspace actions.",
        "Never claim a filesystem change succeeded unless a tool result confirms it.",
    ]
    .join("\n")
}
```

- [ ] **Step 4: Run instruction tests and verify they pass**

Run:

```bash
pixi run cargo test -p agent-runtime instructions:: -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit Task 3**

```bash
git add crates/agent-runtime/src/instructions.rs crates/agent-runtime/src/lib.rs
git commit -m "feat: add instruction context discovery"
```

## Task 4: Integrate ToolRegistry And InstructionContext Into TurnRunner

**Files:**
- Modify: `crates/agent-runtime/src/turn.rs`
- Test: `crates/agent-runtime/src/turn.rs`

Execution granularity:

- Add the fake model tests first and watch them fail.
- Add the new constructor and fields.
- Switch only the tool schema source to `ToolRegistry`, then run tests.
- Switch only model input construction to `InstructionContext`, then run tests.
- Switch only tool execution to `ToolRegistry::execute`, then run tests.
- Add max-tool-call enforcement last and run the turn test suite.

- [ ] **Step 1: Write failing turn-loop integration tests**

Add these tests inside `#[cfg(test)] mod tests` in `crates/agent-runtime/src/turn.rs`:

```rust
struct FilesystemModel {
    calls: AtomicUsize,
    requests: Mutex<Vec<model_gateway::responses::GatewayRequest>>,
}

#[async_trait]
impl ModelClient for FilesystemModel {
    async fn stream(
        &self,
        request: model_gateway::responses::GatewayRequest,
    ) -> anyhow::Result<ModelEventStream> {
        self.requests.lock().unwrap().push(request);
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let events = if call == 0 {
            vec![
                Ok(GatewayEvent::ToolCall {
                    call_id: "call-create".into(),
                    name: "create_directory".into(),
                    arguments: serde_json::json!({ "path": "test" }),
                }),
                Ok(GatewayEvent::Completed),
            ]
        } else {
            vec![
                Ok(GatewayEvent::TextDelta {
                    text: "Created test.".into(),
                }),
                Ok(GatewayEvent::Completed),
            ]
        };
        Ok(Box::pin(stream::iter(events)))
    }
}

#[tokio::test]
async fn executes_builtin_filesystem_tool_and_continues() {
    let workspace = temp_workspace_root("turn-create-directory");
    tokio::fs::create_dir_all(&workspace).await.unwrap();
    let skills = development_skills().await;
    let runner = TurnRunner::new_with_config(
        FilesystemModel {
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
        },
        skills,
        crate::tools::RuntimeConfig::workspace_write(workspace.clone()),
    );

    let events = runner.run("create a test folder").await.unwrap();

    assert!(workspace.join("test").is_dir());
    assert!(events.iter().any(|event| matches!(
        event,
        RuntimeEvent::ToolCallFinished { call_id, result }
            if call_id == "call-create" && result["ok"] == true
    )));
    remove_workspace(workspace).await;
}

#[tokio::test]
async fn first_request_includes_instructions_and_builtin_tool_schema() {
    let workspace = temp_workspace_root("turn-instructions");
    tokio::fs::create_dir_all(&workspace).await.unwrap();
    tokio::fs::write(workspace.join("AGENTS.md"), "Project says use tools.")
        .await
        .unwrap();
    let skills = development_skills().await;
    let runner = TurnRunner::new_with_config(
        FilesystemModel {
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
        },
        skills,
        crate::tools::RuntimeConfig::workspace_write(workspace.clone()),
    );

    let _events = runner.run("create a test folder").await.unwrap();
    let requests = runner.model.requests.lock().unwrap();

    assert_eq!(requests[0].input[0]["role"], "system");
    assert_eq!(requests[0].input[1]["role"], "developer");
    assert!(requests[0].input[1]["content"]
        .as_str()
        .unwrap()
        .contains("Project says use tools."));
    assert!(requests[0].tools.iter().any(|tool| tool.name == "create_directory"));
    assert!(requests[0].tools.iter().any(|tool| tool.name == "echo"));
    remove_workspace(workspace).await;
}

#[tokio::test]
async fn read_only_mode_returns_permission_error_for_write_tool() {
    let workspace = temp_workspace_root("turn-readonly");
    tokio::fs::create_dir_all(&workspace).await.unwrap();
    let skills = development_skills().await;
    let runner = TurnRunner::new_with_config(
        FilesystemModel {
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
        },
        skills,
        crate::tools::RuntimeConfig::read_only(workspace.clone()),
    );

    let events = runner.run("create a test folder").await.unwrap();

    assert!(!workspace.join("test").exists());
    assert!(events.iter().any(|event| matches!(
        event,
        RuntimeEvent::ToolCallFinished { result, .. }
            if result["ok"] == false && result["error"]["code"] == "permission_denied"
    )));
    remove_workspace(workspace).await;
}

struct LoopingToolModel;

#[async_trait]
impl ModelClient for LoopingToolModel {
    async fn stream(
        &self,
        _request: model_gateway::responses::GatewayRequest,
    ) -> anyhow::Result<ModelEventStream> {
        Ok(Box::pin(stream::iter(vec![
            Ok(GatewayEvent::ToolCall {
                call_id: uuid::Uuid::new_v4().to_string(),
                name: "file_metadata".into(),
                arguments: serde_json::json!({ "path": "." }),
            }),
            Ok(GatewayEvent::Completed),
        ])))
    }
}

#[tokio::test]
async fn max_tool_calls_stops_runaway_tool_loop() {
    let workspace = temp_workspace_root("turn-max-tools");
    tokio::fs::create_dir_all(&workspace).await.unwrap();
    let mut config = crate::tools::RuntimeConfig::workspace_write(workspace.clone());
    config.max_tool_calls_per_turn = 2;
    let skills = development_skills().await;
    let runner = TurnRunner::new_with_config(LoopingToolModel, skills, config);

    let events = runner.run("loop forever").await.unwrap();

    assert!(events.iter().any(|event| matches!(
        event,
        RuntimeEvent::TurnFailed { message, .. }
            if message.contains("max tool calls exceeded")
    )));
    remove_workspace(workspace).await;
}

async fn development_skills() -> SkillRegistry {
    let skills_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join("skills");
    SkillRegistry::load(skills_root).await.unwrap()
}

fn temp_workspace_root(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "generalagent-turn-{name}-{}",
        uuid::Uuid::new_v4()
    ))
}

async fn remove_workspace(path: std::path::PathBuf) {
    if path.exists() {
        tokio::fs::remove_dir_all(path).await.unwrap();
    }
}
```

- [ ] **Step 2: Run turn tests and verify they fail**

Run:

```bash
pixi run cargo test -p agent-runtime turn:: -- --nocapture
```

Expected: FAIL with missing `TurnRunner::new_with_config` or missing built-in tool schemas.

- [ ] **Step 3: Update TurnRunner fields and constructors**

Modify the top imports in `crates/agent-runtime/src/turn.rs`:

```rust
use crate::events::RuntimeEvent;
use crate::instructions::{InstructionConfig, InstructionContext};
use crate::skill::SkillRegistry;
use crate::tools::{RuntimeConfig, ToolDefinition, ToolRegistry};
```

Replace the `TurnRunner` struct and constructor with:

```rust
pub struct TurnRunner<C> {
    model: C,
    tools: ToolRegistry,
    config: RuntimeConfig,
    max_steps: usize,
}

impl<C> TurnRunner<C>
where
    C: ModelClient,
{
    pub fn new(model: C, skills: SkillRegistry) -> Self {
        let workspace = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        Self::new_with_config(model, skills, RuntimeConfig::workspace_write(workspace))
    }

    pub fn new_with_config(model: C, skills: SkillRegistry, config: RuntimeConfig) -> Self {
        let tools = ToolRegistry::new(skills, &config);
        Self {
            model,
            tools,
            config,
            max_steps: 8,
        }
    }
```

- [ ] **Step 4: Update TurnRunner input and tool execution**

Inside `TurnRunner::run`, replace initial input and tool setup:

```rust
let instruction_context = InstructionContext::load(InstructionConfig::new(
    self.config.workspace_root.clone(),
    self.config.cwd.clone(),
))
.await?;
let mut input = instruction_context.model_input(user_text);
let tools = gateway_tools(self.tools.definitions());
let mut final_text = String::new();
let mut tool_calls = 0usize;
```

Replace tool execution:

```rust
tool_calls += 1;
if tool_calls > self.config.max_tool_calls_per_turn {
    events.push(RuntimeEvent::TurnFailed {
        turn_id: turn_id.clone(),
        message: "max tool calls exceeded".into(),
    });
    return Ok(events);
}
let result = self.tools.execute(&name, &call_id, arguments).await?;
```

Replace `gateway_tools`:

```rust
fn gateway_tools(tools: Vec<ToolDefinition>) -> Vec<GatewayTool> {
    tools
        .into_iter()
        .map(|tool| GatewayTool {
            name: tool.name,
            description: tool.description,
            input_schema: tool.input_schema,
        })
        .collect()
}
```

- [ ] **Step 5: Run turn tests and verify they pass**

Run:

```bash
pixi run cargo test -p agent-runtime turn:: -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Commit Task 4**

```bash
git add crates/agent-runtime/src/turn.rs
git commit -m "feat: route turns through runtime tools and instructions"
```

## Task 5: Make Completion Endpoint Tool Limits Explicit

**Files:**
- Modify: `crates/model-gateway/src/provider.rs`
- Modify: `crates/model-gateway/src/responses.rs`
- Test: `crates/model-gateway/src/provider.rs`
- Test: `crates/model-gateway/src/responses.rs`

- [ ] **Step 1: Write failing provider capability test**

Add this test inside `#[cfg(test)] mod tests` in `crates/model-gateway/src/provider.rs`:

```rust
#[test]
fn completion_endpoint_does_not_support_tools() {
    let profile = ProviderProfile {
        id: "legacy".into(),
        name: "Legacy".into(),
        endpoint_type: EndpointType::Completion,
        base_url: "http://localhost:11434/v1".into(),
        model: "legacy".into(),
        api_key: None,
        headers: BTreeMap::new(),
    };

    assert_eq!(profile.supports_tools(), false);
}
```

- [ ] **Step 2: Implement provider capability helper**

Add this method to `impl ProviderProfile`:

```rust
pub fn supports_tools(&self) -> bool {
    matches!(
        self.endpoint_type,
        EndpointType::Responses | EndpointType::ChatCompletions
    )
}
```

- [ ] **Step 3: Write failing completion tool rejection test**

Add this test inside `#[cfg(test)] mod tests` in `crates/model-gateway/src/responses.rs`:

```rust
#[test]
fn completion_body_rejects_tool_schemas() {
    let profile = ProviderProfile {
        id: "legacy".into(),
        name: "Legacy".into(),
        endpoint_type: EndpointType::Completion,
        base_url: "http://localhost:11434/v1".into(),
        model: "legacy-model".into(),
        api_key: None,
        headers: BTreeMap::new(),
    };
    let request = GatewayRequest {
        input: vec![serde_json::json!({ "role": "user", "content": "create a folder" })],
        tools: vec![GatewayTool {
            name: "create_directory".into(),
            description: "Create a directory.".into(),
            input_schema: serde_json::json!({ "type": "object" }),
        }],
    };

    let error = gateway_request_body(&profile, request).unwrap_err();

    assert!(error.to_string().contains("model_endpoint_does_not_support_tools"));
}
```

- [ ] **Step 4: Reject completion requests with tools**

Modify `gateway_request_body` in `crates/model-gateway/src/responses.rs`:

```rust
pub fn gateway_request_body(
    profile: &ProviderProfile,
    request: GatewayRequest,
) -> anyhow::Result<Value> {
    if !profile.supports_tools() && !request.tools.is_empty() {
        anyhow::bail!("model_endpoint_does_not_support_tools");
    }

    match profile.endpoint_type {
        EndpointType::Responses => Ok(gateway_responses_body(&profile.model, request)),
        EndpointType::ChatCompletions => {
            responses_to_chat_completions(gateway_base_body(&profile.model, request))
        }
        EndpointType::Completion => Ok(json!({
            "model": profile.model,
            "prompt": latest_text_prompt(&gateway_base_body(&profile.model, request)),
            "stream": false,
        })),
    }
}
```

- [ ] **Step 5: Run model-gateway tests**

Run:

```bash
pixi run cargo test -p model-gateway -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Commit Task 5**

```bash
git add crates/model-gateway/src/provider.rs crates/model-gateway/src/responses.rs
git commit -m "feat: reject tools for completion endpoints"
```

## Task 6: Wire RuntimeConfig Through The Server

**Files:**
- Modify: `crates/agent-server/src/main.rs`
- Modify: `crates/agent-server/src/api.rs`
- Test: `crates/agent-server/src/api.rs`

- [ ] **Step 1: Write the failing server runtime-config test**

Add this test inside `#[cfg(test)] mod tests` in `crates/agent-server/src/api.rs`:

```rust
#[tokio::test]
async fn app_state_accepts_runtime_config_for_model_settings_turns() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let workspace = unique_test_dir("api-runtime-config");
    tokio::fs::create_dir_all(&workspace).await.unwrap();
    let skills = development_skills().await;
    let state = AppState::new_with_agent_and_skills(
        storage,
        Arc::new(DeterministicAgent),
        skills,
    )
    .with_runtime_config(RuntimeConfig::workspace_write(workspace.clone()));

    assert!(state.runtime_config.workspace_root.ends_with(workspace.file_name().unwrap()));
    remove_test_dir(workspace).await;
}
```

If `unique_test_dir`, `development_skills`, or `remove_test_dir` already exist in the test module, reuse their exact existing definitions. If one is missing, add:

```rust
fn unique_test_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "generalagent-server-{name}-{}",
        uuid::Uuid::new_v4()
    ))
}

async fn remove_test_dir(path: PathBuf) {
    if path.exists() {
        tokio::fs::remove_dir_all(path).await.unwrap();
    }
}
```

- [ ] **Step 2: Run the server runtime-config test and verify it fails**

Run:

```bash
pixi run cargo test -p agent-server app_state_accepts_runtime_config_for_model_settings_turns -- --nocapture
```

Expected: FAIL with missing `RuntimeConfig`, `with_runtime_config`, or `runtime_config` field.

- [ ] **Step 3: Add runtime config to AppState**

Modify imports in `crates/agent-server/src/api.rs`:

```rust
use agent_runtime::{
    events::RuntimeEvent,
    session::Message,
    skill::SkillRegistry,
    storage::Storage,
    tools::RuntimeConfig,
    turn::{AgentRunner, TurnRunner},
};
```

Add a field to `AppState`:

```rust
runtime_config: RuntimeConfig,
```

Update constructors:

```rust
pub fn new_with_agent(storage: Storage, agent: Arc<dyn AgentRunner>) -> Self {
    Self {
        storage,
        agent,
        skills: None,
        runtime_config: RuntimeConfig::workspace_write(
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        ),
    }
}

pub fn new_with_agent_and_skills(
    storage: Storage,
    agent: Arc<dyn AgentRunner>,
    skills: SkillRegistry,
) -> Self {
    Self {
        storage,
        agent,
        skills: Some(skills),
        runtime_config: RuntimeConfig::workspace_write(
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        ),
    }
}

pub fn with_runtime_config(mut self, runtime_config: RuntimeConfig) -> Self {
    self.runtime_config = runtime_config;
    self
}
```

- [ ] **Step 4: Use runtime config for per-request model settings runner**

Modify `run_agent_turn`:

```rust
let runner = TurnRunner::new_with_config(
    GatewayHttpClient::new(profile),
    skills,
    state.runtime_config.clone(),
);
```

- [ ] **Step 5: Wire runtime config in main**

Modify imports in `crates/agent-server/src/main.rs`:

```rust
use agent_runtime::{skill::SkillRegistry, storage::Storage, tools::RuntimeConfig, turn::TurnRunner};
```

Add:

```rust
fn runtime_config_from_env() -> anyhow::Result<RuntimeConfig> {
    let workspace_root = std::env::var("GENERAL_AGENT_WORKSPACE_ROOT")
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    Ok(RuntimeConfig::workspace_write(workspace_root))
}
```

In `main`, before building `runner`:

```rust
let runtime_config = runtime_config_from_env()?;
let runner = TurnRunner::new_with_config(model, skills.clone(), runtime_config.clone());
```

And pass config into state:

```rust
let app = api::router(Arc::new(
    api::AppState::new_with_agent_and_skills(storage, Arc::new(runner), skills)
        .with_runtime_config(runtime_config),
));
```

- [ ] **Step 6: Update the configured-provider request assertion for instruction context**

In the existing `post_message_uses_supplied_model_settings_for_agent_turn` test, replace:

```rust
assert_eq!(
    captured.body["messages"][0]["content"],
    "Use the configured provider"
);
```

With:

```rust
let messages = captured.body["messages"].as_array().unwrap();
assert!(messages.iter().any(|message| message["role"] == "system"));
assert!(messages.iter().any(|message| message["role"] == "developer"));
assert!(messages.iter().any(|message| {
    message["role"] == "user" && message["content"] == "Use the configured provider"
}));
```

- [ ] **Step 7: Run server tests**

Run:

```bash
pixi run cargo test -p agent-server api:: -- --nocapture
```

Expected: PASS.

- [ ] **Step 8: Commit Task 6**

```bash
git add crates/agent-server/src/api.rs crates/agent-server/src/main.rs
git commit -m "feat: wire runtime workspace config"
```

## Task 7: Full Verification And Documentation

**Files:**
- Modify: `docs/mvp-verification.md`
- Test: workspace checks

- [ ] **Step 1: Run Rust tests**

Run:

```bash
pixi run cargo test --workspace
```

Expected: PASS for `agent-runtime`, `agent-server`, and `model-gateway`.

- [ ] **Step 2: Run clippy**

Run:

```bash
pixi run cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS with no warnings.

- [ ] **Step 3: Check formatting**

Run:

```bash
pixi run cargo fmt --all --check
```

Expected: PASS. If it fails, run `pixi run cargo fmt --all`, review the formatting diff, then rerun the check.

- [ ] **Step 4: Check source file line counts**

Run:

```bash
wc -l crates/agent-runtime/src/*.rs crates/agent-runtime/src/tools/*.rs crates/model-gateway/src/*.rs crates/agent-server/src/*.rs | sort -n
```

Expected: every edited or created source file is under 1000 physical lines.

- [ ] **Step 5: Update verification docs**

Append this section to `docs/mvp-verification.md`:

```markdown
## Codex-Like Runtime Phase 1 Verification

Date: 2026-06-27

Source design:

- `docs/superpowers/specs/2026-06-27-codex-like-runtime-migration-design.md`

Implementation plan:

- `docs/superpowers/plans/2026-06-27-codex-like-runtime-phase-1.md`

Automated checks:

| Check | Result | Evidence |
| --- | --- | --- |
| `pixi run cargo test --workspace` | PASS | Runtime, server, and gateway tests passed. |
| `pixi run cargo clippy --workspace --all-targets -- -D warnings` | PASS | Clippy completed with no warnings. |
| `pixi run cargo fmt --all --check` | PASS | Rust formatting check passed. |
| Source line budget | PASS | Edited and created source files were checked; all are under 1000 physical lines. |

Runtime behavior verified:

- Built-in `create_directory` can create a workspace directory through the turn loop.
- `read_only` runtime mode blocks write tools with a structured `permission_denied` result.
- The first model request includes base instructions and AGENTS.md project instructions.
- Completion endpoints reject non-empty tool schemas with `model_endpoint_does_not_support_tools`.
```

- [ ] **Step 6: Run whitespace check**

Run:

```bash
git diff --check HEAD
```

Expected: PASS.

- [ ] **Step 7: Commit verification docs**

```bash
git add docs/mvp-verification.md
git commit -m "docs: record codex-like runtime phase 1 verification"
```

## Phase 1 Coverage Matrix

| Spec requirement | Plan coverage | Verification command |
| --- | --- | --- |
| `InstructionContext` module | Task 3, tests `discovers_agents_files_from_workspace_to_cwd`, `renders_model_input_before_user_message`, `truncates_large_agents_file_with_metadata` | `pixi run cargo test -p agent-runtime instructions:: -- --nocapture` |
| AGENTS.md discovery from workspace root to cwd | Task 3 | `pixi run cargo test -p agent-runtime instructions:: -- --nocapture` |
| Tool runtime abstraction | Task 1 and Task 2, `ToolDefinition`, `ToolRegistry`, `RuntimeConfig` | `pixi run cargo test -p agent-runtime tools:: -- --nocapture` |
| `read_only` and `workspace_write` modes | Task 1 permission tests and Task 4 read-only turn test | `pixi run cargo test -p agent-runtime turn:: -- --nocapture` |
| `command_disabled` mode | Task 1 `CommandMode::Disabled` and runtime config default test | `pixi run cargo test -p agent-runtime tools:: -- --nocapture` |
| Built-in filesystem tools | Task 2 built-in tool tests | `pixi run cargo test -p agent-runtime tools::builtin -- --nocapture` |
| Structured tool results | Task 1 result tests and Task 2 structured failure tests | `pixi run cargo test -p agent-runtime tools:: -- --nocapture` |
| Workspace path escape prevention | Task 1 path tests and Task 2 symlink read test | `pixi run cargo test -p agent-runtime tools:: -- --nocapture` |
| Per-tool timeout | Task 2 `runtime_skill_timeout_returns_structured_failure` | `pixi run cargo test -p agent-runtime tools:: -- --nocapture` |
| Output limits | Task 2 `list_directory_applies_entry_limit` and `runtime_skill_output_limit_returns_structured_failure` | `pixi run cargo test -p agent-runtime tools:: -- --nocapture` |
| Max tool calls per turn | Task 4 `max_tool_calls_stops_runaway_tool_loop` | `pixi run cargo test -p agent-runtime turn:: -- --nocapture` |
| Turn loop uses instructions and tools | Task 4 turn integration tests | `pixi run cargo test -p agent-runtime turn:: -- --nocapture` |
| Completion endpoint rejects tools | Task 5 provider and gateway tests | `pixi run cargo test -p model-gateway -- --nocapture` |
| Server runtime config wiring | Task 6 server tests | `pixi run cargo test -p agent-server api:: -- --nocapture` |
| Full workspace verification | Task 7 | `pixi run cargo test --workspace` |

## Self-Review Checklist

Before executing this plan, confirm:

- Phase 1 does not implement `exec_command`, `apply_patch`, MCP, subagents, or approvals.
- Phase 1 does add safe filesystem tools, instruction context, and capability policy.
- Completion endpoint behavior is explicit and tested.
- Existing `skill.json` runtime skills still work.
- Desktop UI remains unchanged.
- No source file is planned to exceed 1000 physical lines.
- Every task has a failing test before implementation.
- Every task has an explicit verification command.
- Every task ends with a commit.
