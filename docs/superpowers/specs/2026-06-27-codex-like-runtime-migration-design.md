# Codex-Like Runtime Migration Design

Date: 2026-06-27

## Goal

Upgrade GeneralAgent from the current MVP agent loop into a Codex-like runtime foundation.

The complete migration should give GeneralAgent the same architectural shape that makes Codex feel agentic:

- A loop that can reason, call tools, observe results, and continue until the task is genuinely handled.
- A model input builder that injects base instructions, developer instructions, project instructions, environment context, skills, and tool guidance.
- A unified tool runtime for built-in tools, runtime skills, future MCP tools, and future app connectors.
- Default developer tools such as command execution, filesystem operations, and patch application.
- A Codex-style skill catalog that can discover `SKILL.md` instructions and expose runtime tools through explicit manifests.
- Safety boundaries for filesystem access, process execution, timeouts, tool errors, and future approval workflows.
- A phased implementation path that keeps each milestone testable and useful on its own.

This design intentionally documents the full target architecture while allowing implementation to proceed in phases.

## Current State

GeneralAgent already has a useful but thin agent loop:

- `crates/agent-runtime/src/turn.rs`
  - `TurnRunner::run` sends model requests.
  - It passes runtime tool schemas from `SkillRegistry`.
  - It executes `GatewayEvent::ToolCall`.
  - It appends the tool result and continues the loop.
- `crates/agent-runtime/src/skill.rs`
  - `SkillRegistry` loads `skill.json` manifests.
  - `SkillRegistry::execute` starts a configured command, writes JSON input to stdin, and expects JSON output from stdout.
- `crates/model-gateway/src/responses.rs`
  - Converts runtime tool schemas into Responses or Chat Completions function tools.
  - Parses function calls from model responses.
- `skills/echo`
  - Provides the only default runtime tool in the repo.

This is enough to prove the architecture, but it is not enough to behave like Codex.

## Observed Gap

When a user asks GeneralAgent to "create a test folder", the current product can easily respond with command-line instructions instead of taking action.

The root causes are:

- No default filesystem tool exists.
- No shell or command execution tool exists.
- No `apply_patch` tool exists.
- The model input does not include Codex-like instructions that say it should use tools for concrete work.
- `AGENTS.md` is not discovered or injected.
- Codex-style `SKILL.md` files are not discovered or injected.
- Runtime skills are only function tools, not instruction skills.
- Tool execution lacks timeouts, structured failures, output limits, and permission boundaries.
- Completion-style providers cannot carry function tool schemas, so tool use silently becomes impossible for that endpoint type.

## Product Positioning

GeneralAgent remains a developer-facing framework for building packaged agent applications.

The end user experience stays simple:

- Chat naturally.
- Configure model connection only if the packaged app chooses to expose that setting.
- Do not manage tools, skills, plugins, or runtime capabilities directly.

The developer experience becomes closer to Codex:

- The developer can give the agent access to workspace-scoped tools.
- The developer can author runtime skills and Codex-style skill instructions.
- The runtime can load project instructions and environment context.
- The framework can package a fixed, hidden capability set into an application.

## Design Principles

- Keep the current Rust runtime and model gateway.
- Preserve the existing function-call loop, but move prompt construction and tool execution into clearer layers.
- Make every phase useful and verifiable.
- Use workspace-scoped safety before attempting a full Codex sandbox.
- Treat Codex as the reference architecture, not as a crate to embed wholesale.
- Keep end-user UI free of skill and tool management concepts.
- Prefer explicit runtime manifests for packaged apps.
- Keep source files below 1000 physical lines by splitting new runtime modules early.

## Target Architecture

```text
Desktop Client / Future Clients
        |
        v
Agent Server
  - session API
  - model settings API
  - dev-only diagnostics API
        |
        v
Turn Orchestrator
  - loop policy
  - max steps
  - cancellation
  - event emission
        |
        +----------------------+
        |                      |
        v                      v
Instruction Context       Tool Runtime
  - base instructions       - built-in tools
  - developer instructions  - runtime skills
  - AGENTS.md stack         - future MCP tools
  - environment context     - future app connectors
  - skill summaries         - safety policies
  - tool guidance           - structured output
        |                      |
        +----------+-----------+
                   |
                   v
Model Gateway
  - Responses
  - Chat Completions
  - Completion compatibility
  - normalized events
```

## Runtime Layers

### Instruction Context

`InstructionContext` owns model-visible context that is not the user's direct message.

Responsibilities:

- Build the base system instruction.
- Add developer instructions.
- Discover and merge project instructions.
- Add environment context, including current working directory, filesystem root, date, and active mode.
- Add available skill summaries.
- Add tool-use guidance.
- Keep instruction blocks stable and testable.

Initial inputs:

- Workspace root.
- Current working directory.
- Optional base instruction override.
- Optional developer instruction string.
- Loaded `AGENTS.md` documents.
- Loaded `SkillCatalog` summaries.
- Enabled tool registry.

The first implementation can represent instruction context as model input items. It does not need to match Codex protocol item-for-item, but the layout should make later parity easier.

### Instruction Authority

Instruction sources must have explicit authority and conflict rules.

Authority order, from strongest to weakest:

1. Framework safety policy.
2. Base system instructions.
3. Developer instructions configured by the host application.
4. Project instructions from `AGENTS.md`.
5. Skill instructions from `SKILL.md`.
6. User message.
7. Tool observations.

Rules:

- Lower-authority instructions cannot weaken workspace restrictions, permission policy, output limits, or endpoint capability checks.
- `AGENTS.md` and `SKILL.md` can guide style, workflow, project conventions, and tool preferences, but cannot enable disabled tools.
- Project and skill instructions must be inserted in labeled blocks that include source path metadata.
- Each instruction file gets an independent byte cap.
- Truncated files must include explicit truncation metadata in the model-visible block.
- Invalid UTF-8 must be handled by lossless replacement or by rejecting the instruction file with a developer diagnostic. Silent corruption is not allowed.
- Instruction blocks must be deterministic so unit tests can compare generated model input.
- Malicious project instructions are treated as project content, not framework policy.

Recommended block shape:

```text
<project_instructions source="AGENTS.md" bytes="2048" truncated="false">
...
</project_instructions>
```

### Project Instructions

GeneralAgent should support project instruction discovery similar to Codex.

Phase 1 discovery:

- Start at the workspace root.
- Read `AGENTS.md` if present.
- Read nested `AGENTS.md` files from workspace root to current working directory.
- Concatenate in directory order so deeper instructions can refine broader instructions.
- Cap each file by byte length to avoid runaway context.
- Preserve UTF-8 text.
- Ignore files outside the workspace root.

Phase 2 discovery:

- Support configurable fallback filenames.
- Support a user-level instruction file if the host application enables it.
- Add metadata that explains which instruction files were included.

### Skill Catalog

`SkillCatalog` is separate from `SkillRegistry`.

`SkillRegistry` remains the runtime function-tool registry:

- Loads `skill.json`.
- Exposes tool schemas.
- Executes runtime tools.
- Is used by packaged apps.

`SkillCatalog` adds Codex-style instruction skill discovery:

- Discovers `SKILL.md`.
- Parses front matter fields such as `name` and `description`.
- Builds a model-visible summary of available skills.
- Loads the full `SKILL.md` only when a skill is selected or mentioned.
- Keeps `references/`, `scripts/`, and `assets/` as development assets unless explicitly packaged.

The two systems can coexist in one skill folder:

```text
skills/
  filesystem/
    SKILL.md
    skill.json
    index.js
    references/
```

The runtime must not assume every `SKILL.md` has a callable function tool. The runtime must not assume every `skill.json` has a useful instruction skill.

Skill lifecycle:

1. Discover summaries from configured skill roots.
2. Render available skill summaries into instruction context.
3. Decide triggers from explicit user mentions, tool/runtime routing hints, or a deterministic selection policy.
4. Load the full `SKILL.md` only for triggered skills.
5. Load referenced instruction files only when the `SKILL.md` routing rules require them.
6. Keep assets and scripts as file references unless a tool call needs to execute or inspect them.
7. Record which skill instructions were injected in developer diagnostics.

Phase 3 trigger policy:

- Exact `$skill-name` user mentions trigger that skill.
- Plain-text mentions can trigger when they match a unique skill name or alias.
- If multiple skills match, inject only summaries and ask the model to proceed with available tool schemas; do not inject all full skill files.
- Runtime tool exposure comes from `skill.json`, not from the trigger decision.
- Packaged apps include instruction skills only when the package manifest explicitly includes them.

### Tool Runtime

`ToolRuntime` becomes the unified execution layer.

Responsibilities:

- Register built-in tools.
- Register runtime skill tools.
- Route tool calls by name.
- Validate arguments before execution.
- Enforce workspace boundaries.
- Enforce timeouts.
- Truncate or summarize large outputs.
- Return structured success and error payloads.
- Emit runtime events for tool start, output, finish, and failure.

The existing `SkillRegistry::execute` can become one executor behind this layer.

### Tool Contracts

Every tool must have a runtime descriptor.

```rust
struct ToolDefinition {
    name: String,
    namespace: Option<String>,
    description: String,
    input_schema: serde_json::Value,
    output_schema: Option<serde_json::Value>,
    permission: ToolPermission,
    source: ToolSource,
}

enum ToolPermission {
    ReadWorkspace,
    WriteWorkspace,
    ExecuteCommand,
    Network,
    ExternalApp,
}

enum ToolSource {
    BuiltIn,
    RuntimeSkill { skill_name: String },
    Mcp { server: String },
    AppConnector { connector: String },
}
```

Tool call execution must preserve:

- `turn_id`
- `call_id`
- tool name
- namespace
- arguments
- start time
- end time
- status
- truncation metadata
- structured error code when failed

The model-visible tool result should use the stable envelope defined in this document. Developer diagnostics can include richer internal fields.

### Turn Orchestrator

`TurnRunner` should evolve into a `TurnOrchestrator`.

Responsibilities:

- Ask `InstructionContext` for model input.
- Ask `ToolRuntime` for model-visible tool schemas.
- Call `ModelClient`.
- Parse text, reasoning, and tool calls.
- Execute tool calls.
- Append tool observations.
- Continue until assistant completion, max steps, cancellation, or failure.
- Keep user-facing assistant text separate from internal runtime events.

The loop structure can remain close to the current code. The important change is that it should no longer directly construct a single bare user message.

### Turn Input and Tool Observation Flow

Each turn should build model input in this order:

1. Base instructions.
2. Developer instructions.
3. Environment context.
4. Project instruction blocks.
5. Skill summary blocks.
6. Selected full skill instruction blocks.
7. Conversation history within budget.
8. Current user message.
9. Prior tool observations for the same turn.

Tool-call continuation flow:

1. Model emits a tool call.
2. Turn orchestrator validates tool name and arguments.
3. Tool runtime checks permission policy and endpoint capability.
4. Tool runtime executes or returns a structured denial.
5. Tool observation is appended to the model input.
6. Model is called again with the same tool registry unless policy changed.

Conversation history should be included only after instruction blocks so project policy remains visible. Later context compaction can summarize old messages, but it must not summarize away active safety policy.

### Model Gateway

The gateway keeps supporting these endpoint types:

- `responses`
- `chat_completions`
- `completion`

Function tools are supported only by endpoint types that can carry tool schemas.

For `completion`, the runtime must use a fixed policy:

- Do not advertise function tools.
- Mark provider capability as `supports_tools = false`.
- Emit a developer diagnostic when tools are configured but unavailable for the endpoint.
- Return `model_endpoint_does_not_support_tools` for turns that explicitly require runtime tools.
- Never silently pretend completion endpoints can execute tool calls.

## Default Tools

The default tools should be introduced in phases.

### Phase 1 Tools

`create_directory`

- Creates a directory under the workspace root.
- Accepts relative paths.
- Rejects absolute paths outside the workspace.
- Rejects path traversal outside the workspace.
- Returns the created path, whether it already existed, and a plain status.

`list_directory`

- Lists a workspace directory with result limits.
- Returns file type, name, relative path, and optional size.
- Refuses paths outside the workspace.
- Sorts output deterministically.

`file_metadata`

- Returns existence, type, size, and modified time.
- Refuses paths outside the workspace.
- Does not follow symlinks outside the workspace.

`read_text_file`

- Reads UTF-8 text under the workspace root.
- Applies output size limits.
- Returns path, content, and truncation metadata.

`write_text_file`

- Writes UTF-8 text under the workspace root.
- Creates parent directories only when explicitly requested.
- Refuses to overwrite unless `overwrite` is true.
- Returns path and byte count.

These tools cover the simplest "do work, not advice" cases without exposing arbitrary shell execution.

### Phase 2 Tools

`search_files`

- Uses `rg` when available through the configured dev environment.
- Accepts pattern, path, and result limits.
- Refuses paths outside the workspace.
- Returns structured matches with truncation metadata.
- Falls back to a safe shell-free implementation when `rg` is missing.

`exec_command`

- Runs a command under a configured working directory.
- Defaults to the workspace root.
- Has timeout and output limits.
- Returns exit code, stdout, stderr, duration, and truncated flags.
- Does not support long-running interactive sessions in the first implementation.
- Does not request escalated permissions in the first implementation.

`apply_patch`

- Applies a structured patch to workspace files.
- Reuses Codex patch grammar when practical.
- Rejects changes outside the workspace.
- Emits a concise summary of changed files.
- Is preferred for source edits over shell-based patch commands.

### Phase 3 Tools

Phase 3 does not need new default filesystem tools. It focuses on Codex-style skill catalog behavior.

### Future Tools

Later phases can add:

- Browser tools.
- Image generation and inspection adapters.
- Spreadsheet, document, and PDF tools.
- MCP connectors.
- App-specific connectors.
- Subagent tools.
- Plugin installation and discovery tools.

These should be added through the same `ToolRuntime` interface.

## Safety Model

The first migration stage should implement practical safety rather than full Codex sandbox parity.

Required in Phase 1:

- Minimal permission model.
- Tool permission metadata.
- `read_only` and `workspace_write` runtime modes.
- Workspace root restriction.
- Canonical path checks.
- No writes outside workspace.
- Per-tool timeouts.
- Output byte limits.
- Structured errors.
- Max tool calls per turn.
- Max agent steps per turn.

Required in Phase 2:

- `command_disabled` and `command_allowed` runtime modes.
- Command tools disabled unless explicitly enabled by development configuration.
- Command timeout.
- Command output truncation.
- Command working directory validation.
- Table-driven command deny rules for known destructive command forms.
- Controlled command environment.
- Process tree termination on timeout where the platform supports it.
- Record command outputs as internal events.

Required in later phases:

- Approval policy.
- Sandbox profiles.
- Network policy.
- Persistent approved command prefixes.
- Consequential tool warnings.
- Per-tool permission metadata.
- Package-time permission declarations.

GeneralAgent should not claim Codex-equivalent sandboxing until those later phases are implemented and verified.

### Capability Policy

Phase 1 must introduce a small capability policy even before full approval support exists.

Modes:

- `read_only`
  - Allows read tools such as `read_text_file`, `list_directory`, and `file_metadata`.
  - Blocks write tools and command tools.
- `workspace_write`
  - Allows read tools and workspace-scoped write tools such as `create_directory` and `write_text_file`.
  - Blocks command tools unless command mode is also enabled.
- `command_disabled`
  - Default command state.
  - `exec_command` is not registered or returns a structured disabled error.
- `command_allowed`
  - Development-only command state in Phase 2.
  - Allows `exec_command` after path, timeout, environment, and deny-rule checks.

Packaged apps choose their mode at build or startup time. Project instructions and skill instructions cannot change it.

### Command Policy

Phase 2 `exec_command` should be intentionally narrower than Codex unified exec.

Rules:

- Input is a command string, matching Codex-style ergonomics.
- The runtime executes through the configured non-login shell.
- Working directory must canonicalize inside the workspace.
- Timeout is required and has a maximum enforced by runtime config.
- Output byte limits apply separately to stdout and stderr.
- Environment starts from a small allowlist plus explicit application-provided variables.
- Network is not sandboxed in Phase 2, so command tools must be development-only by default.
- Deny rules are table-driven and unit-tested.
- Timeout should terminate the process tree where supported.
- Interactive persistent sessions are out of scope for Phase 2.

## Structured Tool Results

Every tool should return JSON with a stable shape.

Runtime representation:

```rust
struct ToolResult {
    ok: bool,
    tool: String,
    call_id: String,
    data: Option<serde_json::Value>,
    error: Option<ToolError>,
    metadata: ToolResultMetadata,
}

struct ToolError {
    code: String,
    message: String,
    retryable: bool,
}

struct ToolResultMetadata {
    duration_ms: u64,
    stdout_truncated: bool,
    stderr_truncated: bool,
    output_truncated: bool,
}
```

Error code families:

- `invalid_arguments`
- `unknown_tool`
- `permission_denied`
- `tool_disabled`
- `path_outside_workspace`
- `path_not_found`
- `path_not_text`
- `model_endpoint_does_not_support_tools`
- `timeout`
- `process_failed`
- `output_limit_exceeded`
- `internal_error`

Success:

```json
{
  "ok": true,
  "tool": "create_directory",
  "call_id": "call-1",
  "data": {
    "path": "test",
    "absolute_path": "/workspace/test",
    "created": true
  },
  "metadata": {
    "duration_ms": 4,
    "stdout_truncated": false,
    "stderr_truncated": false,
    "output_truncated": false
  }
}
```

Failure:

```json
{
  "ok": false,
  "tool": "create_directory",
  "call_id": "call-1",
  "error": {
    "code": "path_outside_workspace",
    "message": "Path must stay inside the workspace.",
    "retryable": false
  },
  "metadata": {
    "duration_ms": 1,
    "stdout_truncated": false,
    "stderr_truncated": false,
    "output_truncated": false
  }
}
```

The model sees the tool result. The UI can also store and inspect internal events, but normal end users should not see raw technical tool JSON unless the packaged app intentionally exposes developer diagnostics.

## Events

The existing `RuntimeEvent` enum should expand without breaking current consumers.

Current events:

- `turn_started`
- `assistant_text_delta`
- `reasoning_delta`
- `tool_call_started`
- `tool_call_finished`
- `assistant_message_finished`
- `turn_finished`
- `turn_failed`

Future events:

- `tool_call_failed`
- `tool_output_delta`
- `instruction_context_built`
- `tool_registry_built`
- `turn_cancelled`
- `usage_reported`

Events should remain serializable to snake_case JSON.

### Event Visibility

Runtime data has three views.

Public response view:

- User message.
- Final assistant message.
- Turn status.
- Sanitized error message when the turn fails.
- No raw command output, full tool arguments, full tool result payload, or internal paths unless the packaged app opts into developer mode.

Developer diagnostics view:

- Full runtime events.
- Tool arguments after redaction.
- Tool results after output limits.
- Instruction source metadata.
- Tool registry metadata.
- Endpoint capability diagnostics.

Persistent event view:

- Stable event id.
- Session id.
- Turn id.
- Optional call id.
- Event type.
- Redacted payload.
- Schema version.
- Created timestamp.

The current local API can keep returning runtime events during development, but packaged production should default to the public response view.

## Storage

The storage layer should eventually distinguish:

- User-visible messages.
- Assistant-visible final text.
- Reasoning summaries.
- Tool calls.
- Tool results.
- Runtime diagnostics.
- Usage metadata.

Phase 1 can keep the existing message table and return runtime events in API responses. Persistent tool event storage can be a later phase.

When persistent runtime events are added, stored payloads must be versioned and redacted according to the event visibility rules.

## API Design

Production APIs remain simple:

- `POST /sessions`
- `POST /sessions/:session_id/messages`
- Model test and model settings endpoints.
- Future session history endpoints.

Dev-only APIs can be added behind an explicit development flag:

- `GET /dev/tools`
- `GET /dev/skills`
- `POST /dev/skills/validate`
- `POST /dev/instructions/preview`

Production builds must not expose skill or tool management as an end-user product feature.

## Desktop UI

No new user-facing tool or skill UI is required for the migration foundation.

The desktop app should continue to show:

- Chat.
- Conversation drawer.
- Model connection settings.

It should not show:

- Skills tab.
- Tool inventory.
- Capability chips.
- Raw tool-call panels.
- Marketplace concepts.

Developer diagnostics can be added later behind explicit dev mode.

## Packaging

Packaged apps should freeze runtime capabilities.

Development mode:

- Scans local `skills/`.
- Scans optional Codex-style skill roots.
- Allows restart-based reload.
- Supports diagnostics.

Packaged mode:

- Loads a generated `skill-bundle.json`.
- Loads only approved runtime files.
- Treats tool inventory as immutable.
- Excludes development-only instruction assets by default.
- Includes `SKILL.md` only when the app explicitly enables instruction-skill packaging.

Packaging checks:

- Validate `skill.json`.
- Validate tool names.
- Validate JSON schemas.
- Validate entry command references.
- Validate `SKILL.md` front matter when included.
- Validate all packaged paths remain inside the package root.

Minimal packaged bundle manifest:

```json
{
  "schema_version": 1,
  "generated_at": "2026-06-27T00:00:00Z",
  "permissions": {
    "mode": "workspace_write",
    "commands": "disabled",
    "network": "disabled"
  },
  "skills": [
    {
      "name": "filesystem",
      "path": "skills/filesystem",
      "runtime_manifest": "skills/filesystem/skill.json",
      "instruction_manifest": "skills/filesystem/SKILL.md",
      "include_instructions": false,
      "files": [
        {
          "path": "skills/filesystem/skill.json",
          "sha256": "example"
        },
        {
          "path": "skills/filesystem/index.js",
          "sha256": "example"
        }
      ],
      "permissions": ["read_workspace", "write_workspace"]
    }
  ]
}
```

Bundle trust rules:

- Paths are relative to the package root.
- Absolute paths are invalid.
- Parent traversal is invalid.
- Hashes are verified when present.
- Runtime entry commands must reference packaged files or known runtime executables.
- Development assets are excluded unless `include_instructions` or an equivalent package flag includes them.
- Permission declarations are package-time metadata and cannot be expanded by `AGENTS.md` or `SKILL.md`.

## Phased Roadmap

### Phase 0: Documentation and Architecture Lock

Deliverables:

- This complete migration design.
- A phased implementation plan.
- Clear acceptance criteria for each phase.

Acceptance criteria:

- The design explains current gaps, target architecture, default tools, safety model, skills, instructions, APIs, packaging, and tests.
- The first implementation phase is small enough to complete without rewriting the whole runtime.

### Phase 1: Runtime Foundation and Safe Filesystem Tools

Deliverables:

- `InstructionContext` module.
- AGENTS.md discovery from workspace root to cwd.
- `ToolRuntime` trait or equivalent executor abstraction.
- Minimal capability policy with `read_only`, `workspace_write`, and `command_disabled`.
- Built-in `create_directory`, `list_directory`, `file_metadata`, `read_text_file`, and `write_text_file`.
- Structured tool success and failure payloads.
- Updated turn loop input construction.
- Tests proving "create a test folder" can execute through a tool.

Acceptance criteria:

- A model that emits `create_directory` creates the directory under the workspace.
- The runtime refuses paths outside the workspace.
- The first model request includes base instructions and project instructions.
- `read_only` mode blocks write tools.
- Existing `echo` skill tests still pass.
- No edited source file exceeds 1000 physical lines.

### Phase 2: Command and Patch Tools

Deliverables:

- `search_files`.
- Workspace-scoped `exec_command`.
- Development-only `command_allowed` mode.
- Table-driven command deny rules.
- Non-interactive command execution with timeout.
- Output truncation metadata.
- Minimal `apply_patch` support.
- Tests for search results, command success, command failure, timeout, disabled command mode, deny rules, and outside-workspace patch rejection.

Acceptance criteria:

- The agent can search files without falling back to shell.
- The agent can run simple workspace commands through a tool.
- The agent can edit files through `apply_patch`.
- Command output is structured and bounded.
- Obvious path escape attempts fail.

### Phase 3: Codex-Style Skill Catalog

Deliverables:

- `SkillCatalog` module.
- `SKILL.md` front matter parsing.
- Available skill summary injection.
- Mentioned or selected skill instruction loading.
- Compatibility with existing `skill.json`.
- Tests for skill discovery, summary injection, and full instruction injection.

Acceptance criteria:

- Runtime skills and instruction skills can coexist.
- A skill without `skill.json` can still provide instructions.
- A runtime tool without `SKILL.md` can still execute.
- Packaged mode can exclude `SKILL.md` unless explicitly configured.

### Phase 4: Tool Registry Expansion and Diagnostics

Deliverables:

- Tool registry inspection for dev mode.
- Dev-only `/dev/tools` and `/dev/instructions/preview`.
- Tool schema validation improvements.
- Tool result output schema checks where declared.
- Persistent runtime diagnostics if storage schema is expanded in this phase.

Acceptance criteria:

- Development diagnostics can explain what tools and instructions are visible to the model.
- Production API does not expose dev diagnostics by default.
- Invalid tool schemas fail fast during startup or packaging.

### Phase 5: Approval and Sandbox Foundations

Deliverables:

- Approval workflow for permission transitions.
- Approval policy data structures.
- Stronger sandbox profiles.
- Network access policy placeholder.
- Structured events for approval-required outcomes.

Acceptance criteria:

- The runtime can request approval without executing blocked actions.
- Approval decisions are represented in structured events.
- Sandbox profiles are explicit about filesystem and network behavior.
- The API can report that an action requires approval without leaking raw tool payloads.

### Phase 6: MCP, Connectors, and Deferred Tools

Deliverables:

- MCP tool adapter interface.
- Namespaced tool names.
- Deferred tool discovery.
- Connector metadata model.
- Tool search or discovery endpoint for dev mode.

Acceptance criteria:

- Built-in tools, runtime skills, and MCP tools can share one registry.
- Namespaced tools do not collide with built-ins.
- Deferred tools can be advertised without loading every full schema into every turn.

### Phase 7: Advanced Codex Parity

Deliverables:

- Streaming tool-call argument handling.
- Parallel tool call execution where safe.
- Subagent integration.
- Richer context compaction.
- Goal and budget extensions.
- Plugin installation workflow if the product chooses to support it.

Acceptance criteria:

- Long-running tasks can continue through multiple tool calls with bounded context.
- Parallel execution is limited to independent tool calls.
- Subagents are represented as tools or runtime services with clear lifecycle events.

## Testing Strategy

### Unit Tests

Add unit tests for:

- AGENTS.md discovery order.
- Instruction context construction.
- Instruction authority and conflict handling.
- Workspace path canonicalization.
- Capability policy decisions.
- Built-in tool argument validation.
- Structured tool result shapes.
- Tool registry routing.
- Skill manifest loading.
- `SKILL.md` front matter parsing.
- Gateway conversion of tool schemas.
- Packaged bundle manifest validation.

### Runtime Tests

Add runtime tests for:

- Model emits a filesystem tool call and the runtime executes it.
- Model receives tool output and continues to final assistant text.
- Tool failure is returned to the model and represented in events.
- Max steps stops runaway loops.
- Completion endpoint cannot silently use tools.
- `read_only` mode blocks write tools.
- Completion endpoint reports `model_endpoint_does_not_support_tools` for tool-required turns.

### Integration Tests

Add server tests for:

- Posting a message that triggers a tool call.
- Preserving user-visible assistant text.
- Returning runtime events.
- Keeping production APIs free of public skill management.
- Dev-only diagnostics disabled by default.
- Public event view does not expose raw command output or full tool arguments by default.

### Manual Verification

Manual checks should cover:

- Creating a folder from natural language.
- Reading and writing a small text file.
- Rejecting path traversal.
- Loading project AGENTS.md.
- Confirming desktop UI still has no user-facing tool or skill management.

## Migration From Current Code

The migration should avoid a large rewrite.

Recommended file additions:

- `crates/agent-runtime/src/instructions.rs`
- `crates/agent-runtime/src/tools/mod.rs`
- `crates/agent-runtime/src/tools/builtin.rs`
- `crates/agent-runtime/src/tools/path.rs`
- `crates/agent-runtime/src/tools/result.rs`
- `crates/agent-runtime/src/skill_catalog.rs`

Recommended file changes:

- `crates/agent-runtime/src/lib.rs`
  - Export new modules.
- `crates/agent-runtime/src/turn.rs`
  - Depend on instruction and tool runtime abstractions.
- `crates/agent-runtime/src/skill.rs`
  - Keep manifest loading and command skill execution.
- `crates/agent-server/src/main.rs`
  - Build runtime config with workspace root and dev mode.
- `crates/agent-server/src/api.rs`
  - Keep production API stable.
- `crates/model-gateway/src/responses.rs`
  - Make unsupported tool endpoints explicit.

The first implementation should not edit desktop UI unless runtime events require type updates.

## Backward Compatibility

The migration should preserve:

- Existing chat API shape where practical.
- Existing model settings behavior.
- Existing `echo` skill tests.
- Existing `skill.json` format.
- Existing desktop user experience.

Breaking changes are acceptable only for internal runtime APIs that are not exposed to packaged app users.

## Non-Goals

The complete migration does not require these in the first phases:

- Full Codex CLI compatibility.
- Full Codex TUI compatibility.
- OpenAI account login.
- Codex cloud task integration.
- Full Seatbelt or Linux sandbox parity.
- Interactive persistent shell sessions.
- User-facing skill marketplace.
- User-facing plugin installation.
- Public end-user skill toggles.

These may be future product decisions, but they are not required for the Codex-like runtime foundation.

## Risks

### Scope Creep

Codex is broad. The migration can easily become a rewrite.

Mitigation:

- Implement phase by phase.
- Keep acceptance criteria small.
- Treat complete parity as a roadmap, not a single milestone.

### Unsafe Tool Execution

Adding command and filesystem tools introduces real side effects.

Mitigation:

- Start with safe filesystem tools before shell.
- Enforce workspace boundaries.
- Add timeout and output limits.
- Add approval and sandbox work in later phases before claiming stronger safety.

### Prompt Instability

Adding instructions and skill summaries can change model behavior.

Mitigation:

- Keep instruction blocks deterministic.
- Test generated model requests with fake models.
- Add preview diagnostics in dev mode.

### Provider Differences

Some providers do not support function tools or reason about tool schemas poorly.

Mitigation:

- Make endpoint limitations explicit.
- Add provider capability metadata.
- Keep tests for Responses and Chat Completions separately.

### File Growth

`api.rs`, `skill.rs`, and `turn.rs` are already large enough that adding features directly would create maintenance risk.

Mitigation:

- Split new modules early.
- Keep source files below 1000 physical lines.
- Move path safety, tool results, and instruction building into dedicated files.

## Documentation Requirements

Implementation docs should include:

- A phase-specific implementation plan.
- A verification record for each completed phase.
- Any deviations from this design.
- Exact commands used for tests through `pixi`.
- Source file line-count confirmation for edited source files.

The final implementation should update `docs/mvp-verification.md` or create a new verification document for the runtime migration.

## First Implementation Recommendation

Start with Phase 1.

Phase 1 is the smallest slice that addresses the original user-visible failure:

- The model can call a default filesystem tool.
- The runtime can execute the requested action.
- The result returns to the model loop.
- Project instructions can tell the model to act through tools.

This gives GeneralAgent a real Codex-like foundation without prematurely importing the hardest parts of Codex: sandboxing, approvals, MCP, subagents, and plugin installation.
