# Codex-Like Runtime Phase 7 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the Phase 7 advanced Codex parity foundation: streaming tool argument assembly, turn-local goal/budget handling, deterministic context compaction, subagent lifecycle events, and conservative parallel-safety metadata.

**Architecture:** Avoid a rewrite of `TurnRunner`. Add small modules for streaming assembly, turn requests, compaction, subagent lifecycle, and tool execution policy. Keep real SSE, real subagent workers, persistent budgets, plugin install, and model-summarized compaction out of scope. Split near-limit files before adding behavior.

**Tech Stack:** Rust 2024, futures/tokio, serde/serde_json, existing `GatewayEvent`, `RuntimeEvent`, `TurnRunner`, `ToolRegistry`, pixi-managed cargo commands.

---

## Scope

Phase 7 scope from the migration design:

- Streaming tool-call argument handling foundation.
- Parallel-safe tool-call policy metadata.
- Turn-local goal and budget request model.
- Usage event accounting from `GatewayEvent::Usage`.
- Deterministic context compaction that preserves instruction authority blocks.
- Subagent lifecycle service shell and structured events.

Out of scope:

- Real HTTP SSE streaming.
- Real parallel execution scheduler for side-effecting tools.
- Real external subagent process management or desktop UI.
- Persistent goal/budget storage.
- Model-summarized compaction.
- Plugin installation workflow.

## File Structure

Create:

- `crates/agent-runtime/src/tools/registry_tests.rs`
  - Moves `tools/mod.rs` tests out of the near-limit source file.
- `crates/model-gateway/src/streaming.rs`
  - Owns streaming tool-call argument assembly.
- `crates/agent-runtime/src/turn_request.rs`
  - Owns `TurnRequest`, `TurnGoal`, and `BudgetPolicy`.
- `crates/agent-runtime/src/context.rs`
  - Owns deterministic context compaction helpers.
- `crates/agent-runtime/src/subagent.rs`
  - Owns the fake/local subagent lifecycle service shell.

Modify:

- `crates/agent-runtime/src/lib.rs`
  - Export new runtime modules.
- `crates/model-gateway/src/lib.rs`
  - Export `streaming`.
- `crates/agent-runtime/src/events.rs`
  - Add usage, compaction, and subagent lifecycle events.
- `crates/agent-runtime/src/tools/mod.rs`
  - Add `ToolRegistry::parallel_safe(name)`.
  - Replace inline test module with `mod registry_tests`.
- `crates/agent-runtime/src/turn.rs`
  - Add `TurnRunner::run_request`.
  - Inject active goal context.
  - Report usage and stop on token budget.
  - Emit context compaction events when deterministic budget compaction runs.
- `docs/mvp-verification.md`
  - Append Phase 7 verification evidence.

## Task 1: Tool Registry Test Split

**Files:**
- Create: `crates/agent-runtime/src/tools/registry_tests.rs`
- Modify: `crates/agent-runtime/src/tools/mod.rs`

- [x] **Step 1: Run current tools tests as refactor baseline**

Run:

```bash
pixi run cargo test -p agent-runtime tools::tests:: -- --nocapture
wc -l crates/agent-runtime/src/tools/mod.rs
```

Expected: tests pass; `tools/mod.rs` is close to 1000 lines.

- [x] **Step 2: Move tests without behavior changes**

Replace the inline module footer in `tools/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    /* current tests */
}
```

with:

```rust
#[cfg(test)]
mod registry_tests;
```

Move the test module body to `crates/agent-runtime/src/tools/registry_tests.rs` and start it with:

```rust
use super::*;
use crate::skill::SkillRegistry;
use std::path::PathBuf;
```

Keep test names unchanged.

- [x] **Step 3: Run moved tests**

Run:

```bash
pixi run cargo test -p agent-runtime tools::registry_tests:: -- --nocapture
pixi run cargo fmt --all --check
wc -l crates/agent-runtime/src/tools/mod.rs crates/agent-runtime/src/tools/registry_tests.rs
```

Expected: tests pass; both source files are below 1000 physical lines.

- [x] **Step 4: Commit test split**

Run:

```bash
git add crates/agent-runtime/src/tools/mod.rs crates/agent-runtime/src/tools/registry_tests.rs
git commit -m "refactor: split tool registry tests"
```

## Task 2: Streaming Tool Argument Assembly

**Files:**
- Create: `crates/model-gateway/src/streaming.rs`
- Modify: `crates/model-gateway/src/lib.rs`

- [x] **Step 1: Write failing streaming assembler tests**

Add tests in `streaming.rs`:

```rust
#[test]
fn assembles_streaming_tool_arguments_before_emitting_call() {
    let mut assembler = StreamingToolCallAssembler::default();

    assert!(assembler
        .push(ToolCallDelta {
            index: 0,
            call_id: Some("call-1".into()),
            name: Some("create_directory".into()),
            arguments_delta: Some("{\"path\"".into()),
        })
        .unwrap()
        .is_none());
    let completed = assembler
        .push(ToolCallDelta {
            index: 0,
            call_id: None,
            name: None,
            arguments_delta: Some(":\"test\"}".into()),
        })
        .unwrap()
        .unwrap();

    assert_eq!(completed.call_id, "call-1");
    assert_eq!(completed.name, "create_directory");
    assert_eq!(completed.arguments["path"], "test");
}

#[test]
fn assembler_keeps_parallel_tool_indices_separate() {
    let mut assembler = StreamingToolCallAssembler::default();

    assembler.push(ToolCallDelta {
        index: 0,
        call_id: Some("call-a".into()),
        name: Some("read_text_file".into()),
        arguments_delta: Some("{\"path\":\"a.txt\"}".into()),
    }).unwrap();
    assembler.push(ToolCallDelta {
        index: 1,
        call_id: Some("call-b".into()),
        name: Some("read_text_file".into()),
        arguments_delta: Some("{\"path\":\"b.txt\"}".into()),
    }).unwrap();

    let calls = assembler.finish_all().unwrap();

    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].arguments["path"], "a.txt");
    assert_eq!(calls[1].arguments["path"], "b.txt");
}
```

- [x] **Step 2: Run streaming tests to verify they fail**

Run:

```bash
pixi run cargo test -p model-gateway streaming::tests -- --nocapture
```

Expected: fail because the streaming module does not exist.

- [x] **Step 3: Implement assembler**

Add:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallDelta {
    pub index: usize,
    pub call_id: Option<String>,
    pub name: Option<String>,
    pub arguments_delta: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompletedToolCall {
    pub call_id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Default)]
pub struct StreamingToolCallAssembler {
    calls: BTreeMap<usize, PartialToolCall>,
}
```

`push` accumulates deltas and returns `Some(CompletedToolCall)` only when arguments parse as valid JSON and call id/name are known. `finish_all` parses every accumulated call and returns them sorted by index.

Export in `model-gateway/src/lib.rs`:

```rust
pub mod streaming;
```

- [x] **Step 4: Run streaming tests to verify they pass**

Run:

```bash
pixi run cargo test -p model-gateway streaming::tests -- --nocapture
```

- [x] **Step 5: Commit streaming assembler**

Run:

```bash
git add crates/model-gateway/src/streaming.rs crates/model-gateway/src/lib.rs
git commit -m "feat: add streaming tool call assembler"
```

## Task 3: Turn Request Goal and Usage Budget

**Files:**
- Create: `crates/agent-runtime/src/turn_request.rs`
- Modify: `crates/agent-runtime/src/lib.rs`
- Modify: `crates/agent-runtime/src/events.rs`
- Modify: `crates/agent-runtime/src/turn.rs`

- [x] **Step 1: Write failing goal and usage tests**

Add tests in `turn_request.rs`:

```rust
#[test]
fn budget_policy_tracks_total_tokens() {
    let mut budget = BudgetPolicy::new(Some(10));

    assert!(!budget.record_usage(3, 4).exceeded);
    let usage = budget.record_usage(2, 2);

    assert_eq!(usage.total_tokens, 11);
    assert!(usage.exceeded);
}
```

Add tests in `turn.rs`:

```rust
#[tokio::test]
async fn goal_context_is_injected_into_turn_input() {
    let workspace = test_workspace("goal-context");
    let skills = SkillRegistry::load(skills_root()).await.unwrap();
    let runner = TurnRunner::new_with_config(
        ScriptedModel {
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            responses: vec![vec![GatewayEvent::Completed]],
        },
        skills,
        RuntimeConfig::workspace_write(workspace.clone(), workspace.clone()),
    );

    let request = crate::turn_request::TurnRequest::new("continue")
        .with_goal(crate::turn_request::TurnGoal::new("finish phase 7"));
    let _events = runner.run_request(request).await.unwrap();
    let requests = runner.model.requests.lock().unwrap();

    assert!(requests[0].input.iter().any(|item| {
        item["content"].as_str().unwrap_or_default().contains("<active_goal>")
    }));
    remove_workspace(&workspace);
}

#[tokio::test]
async fn usage_budget_accumulates_gateway_usage_and_stops_turn() {
    let workspace = test_workspace("usage-budget");
    let skills = SkillRegistry::load(skills_root()).await.unwrap();
    let runner = TurnRunner::new_with_config(
        ScriptedModel {
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            responses: vec![vec![
                GatewayEvent::Usage { input_tokens: 6, output_tokens: 5 },
                GatewayEvent::Completed,
            ]],
        },
        skills,
        RuntimeConfig::workspace_write(workspace.clone(), workspace.clone()),
    );

    let request = crate::turn_request::TurnRequest::new("hello").with_token_budget(10);
    let events = runner.run_request(request).await.unwrap();

    assert!(events.iter().any(|event| matches!(
        event,
        RuntimeEvent::UsageReported { total_tokens: 11, exceeded: true, .. }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        RuntimeEvent::TurnFailed { message, .. } if message.contains("token budget exceeded")
    )));
    remove_workspace(&workspace);
}
```

- [x] **Step 2: Run goal/budget tests to verify they fail**

Run:

```bash
pixi run cargo test -p agent-runtime turn_request::tests -- --nocapture
pixi run cargo test -p agent-runtime turn::tests::goal_context_is_injected_into_turn_input -- --nocapture
pixi run cargo test -p agent-runtime turn::tests::usage_budget_accumulates_gateway_usage_and_stops_turn -- --nocapture
```

Expected: fail because turn requests, usage events, and goal injection do not exist.

- [x] **Step 3: Implement turn request and usage events**

Add:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnGoal {
    pub objective: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnRequest {
    pub user_text: String,
    pub goal: Option<TurnGoal>,
    pub token_budget: Option<u64>,
    pub context_budget_bytes: Option<usize>,
}
```

Add `RuntimeEvent::UsageReported { input_tokens, output_tokens, total_tokens, exceeded }`.

Implement `TurnRunner::run_request(request)` and have `run(user_text)` call it with `TurnRequest::new(user_text)`.

On `GatewayEvent::Usage`, accumulate budget, emit `UsageReported`, and return `TurnFailed` when the request budget is exceeded.

- [x] **Step 4: Run goal/budget tests to verify they pass**

Run:

```bash
pixi run cargo test -p agent-runtime turn_request::tests -- --nocapture
pixi run cargo test -p agent-runtime turn::tests::goal_context_is_injected_into_turn_input -- --nocapture
pixi run cargo test -p agent-runtime turn::tests::usage_budget_accumulates_gateway_usage_and_stops_turn -- --nocapture
```

- [x] **Step 5: Commit turn request and budget**

Run:

```bash
git add crates/agent-runtime/src/turn_request.rs crates/agent-runtime/src/lib.rs crates/agent-runtime/src/events.rs crates/agent-runtime/src/turn.rs
git commit -m "feat: add turn goals and usage budgets"
```

## Task 4: Context Compaction Foundation

**Files:**
- Create: `crates/agent-runtime/src/context.rs`
- Modify: `crates/agent-runtime/src/lib.rs`
- Modify: `crates/agent-runtime/src/events.rs`
- Modify: `crates/agent-runtime/src/turn.rs`

- [x] **Step 1: Write failing compaction tests**

Add tests in `context.rs`:

```rust
#[test]
fn compaction_preserves_authority_blocks_and_current_user() {
    let input = vec![
        json!({ "role": "system", "content": "system policy" }),
        json!({ "role": "developer", "content": "developer policy" }),
        json!({ "role": "user", "content": "old user" }),
        json!({ "role": "assistant", "content": "old answer" }),
        json!({ "role": "user", "content": "current user" }),
    ];

    let compacted = compact_model_input(input, 160).unwrap();

    assert_eq!(compacted[0]["content"], "system policy");
    assert_eq!(compacted[1]["content"], "developer policy");
    assert!(compacted.iter().any(|item| {
        item["content"].as_str().unwrap_or_default().contains("<context_compaction>")
    }));
    assert_eq!(compacted.last().unwrap()["content"], "current user");
}
```

Add a turn test:

```rust
#[tokio::test]
async fn context_compaction_emits_event_when_budget_applies() {
    let workspace = test_workspace("context-compaction");
    let skills = SkillRegistry::load(skills_root()).await.unwrap();
    let runner = TurnRunner::new_with_config(
        ScriptedModel {
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            responses: vec![vec![GatewayEvent::Completed]],
        },
        skills,
        RuntimeConfig::workspace_write(workspace.clone(), workspace.clone()),
    );

    let request = crate::turn_request::TurnRequest::new("hello").with_context_budget_bytes(64);
    let events = runner.run_request(request).await.unwrap();

    assert!(events.iter().any(|event| matches!(
        event,
        RuntimeEvent::ContextCompacted { .. }
    )));
    remove_workspace(&workspace);
}
```

- [x] **Step 2: Run compaction tests to verify they fail**

Run:

```bash
pixi run cargo test -p agent-runtime context::tests -- --nocapture
pixi run cargo test -p agent-runtime turn::tests::context_compaction_emits_event_when_budget_applies -- --nocapture
```

- [x] **Step 3: Implement deterministic compaction**

Add `compact_model_input(input, budget_bytes)` that:

- Keeps leading `system` and `developer` items.
- Keeps the final user item.
- Replaces middle items with one developer item containing `<context_compaction>` and counts.
- Never removes authority blocks.

Add `RuntimeEvent::ContextCompacted { original_items, compacted_items, budget_bytes }`.

Call compaction in `TurnRunner::run_request` after goal injection and before the first model request when `context_budget_bytes` is set.

- [x] **Step 4: Run compaction tests to verify they pass**

Run:

```bash
pixi run cargo test -p agent-runtime context::tests -- --nocapture
pixi run cargo test -p agent-runtime turn::tests::context_compaction_emits_event_when_budget_applies -- --nocapture
```

- [x] **Step 5: Commit compaction foundation**

Run:

```bash
git add crates/agent-runtime/src/context.rs crates/agent-runtime/src/lib.rs crates/agent-runtime/src/events.rs crates/agent-runtime/src/turn.rs
git commit -m "feat: add deterministic context compaction"
```

## Task 5: Subagent Lifecycle Shell

**Files:**
- Create: `crates/agent-runtime/src/subagent.rs`
- Modify: `crates/agent-runtime/src/lib.rs`
- Modify: `crates/agent-runtime/src/events.rs`

- [x] **Step 1: Write failing subagent lifecycle tests**

Add tests in `subagent.rs`:

```rust
#[tokio::test]
async fn subagent_service_emits_started_and_finished_events() {
    let service = SubagentService::default();

    let events = service.run_fake_task("review phase 7").await;

    assert!(matches!(events[0], RuntimeEvent::SubagentStarted { .. }));
    assert!(matches!(events[1], RuntimeEvent::SubagentFinished { .. }));
}

#[tokio::test]
async fn subagent_service_emits_failed_on_error() {
    let service = SubagentService::default();

    let events = service.fail_fake_task("review phase 7", "timeout").await;

    assert!(events.iter().any(|event| matches!(
        event,
        RuntimeEvent::SubagentFailed { message, .. } if message == "timeout"
    )));
}
```

- [x] **Step 2: Run subagent tests to verify they fail**

Run:

```bash
pixi run cargo test -p agent-runtime subagent::tests -- --nocapture
```

- [x] **Step 3: Implement subagent service shell**

Add `RuntimeEvent` variants:

```rust
SubagentStarted { subagent_id: String, task: String },
SubagentFinished { subagent_id: String },
SubagentFailed { subagent_id: String, message: String },
```

Add `SubagentService` that returns deterministic lifecycle event vectors for fake local tasks. This is a runtime service shell only; it does not spawn real agents.

- [x] **Step 4: Run subagent tests to verify they pass**

Run:

```bash
pixi run cargo test -p agent-runtime subagent::tests -- --nocapture
```

- [x] **Step 5: Commit subagent lifecycle shell**

Run:

```bash
git add crates/agent-runtime/src/subagent.rs crates/agent-runtime/src/lib.rs crates/agent-runtime/src/events.rs
git commit -m "feat: add subagent lifecycle shell"
```

## Task 6: Parallel-Safe Tool Policy

**Files:**
- Modify: `crates/agent-runtime/src/tools/mod.rs`
- Modify: `crates/agent-runtime/src/tools/registry_tests.rs`

- [x] **Step 1: Write failing parallel policy tests**

Add tests in `tools/registry_tests.rs`:

```rust
#[test]
fn read_only_builtin_tools_are_parallel_safe() {
    let root = unique_test_dir("parallel-safe-read");
    std::fs::create_dir_all(&root).unwrap();
    let registry = ToolRegistry::new(
        SkillRegistry::empty_for_tests(),
        &RuntimeConfig::workspace_write(root.clone(), root.clone()),
    );

    assert!(registry.parallel_safe("read_text_file"));
    assert!(registry.parallel_safe("list_directory"));
}

#[test]
fn write_command_runtime_and_external_tools_are_not_parallel_safe_by_default() {
    let root = unique_test_dir("parallel-unsafe");
    std::fs::create_dir_all(&root).unwrap();
    let config = RuntimeConfig {
        external_tools: vec![crate::tools::discovery::ExternalToolConfig::mcp(
            "search",
            "lookup",
            "Search.",
            serde_json::json!({ "type": "object" }),
            crate::tools::discovery::ExternalToolVisibility::Immediate,
        )],
        ..RuntimeConfig::workspace_write(root.clone(), root.clone())
    };
    let registry = ToolRegistry::new(SkillRegistry::empty_for_tests(), &config);

    assert!(!registry.parallel_safe("create_directory"));
    assert!(!registry.parallel_safe("mcp__search__lookup"));
}
```

- [x] **Step 2: Run parallel policy tests to verify they fail**

Run:

```bash
pixi run cargo test -p agent-runtime tools::registry_tests::read_only_builtin_tools_are_parallel_safe -- --nocapture
pixi run cargo test -p agent-runtime tools::registry_tests::write_command_runtime_and_external_tools_are_not_parallel_safe_by_default -- --nocapture
```

- [x] **Step 3: Implement conservative parallel policy**

Add:

```rust
impl ToolRegistry {
    pub fn parallel_safe(&self, name: &str) -> bool {
        self.definitions().into_iter().any(|definition| {
            definition.name == name
                && definition.permission == ToolPermission::ReadWorkspace
                && matches!(definition.source, ToolSource::BuiltIn)
        })
    }
}
```

This is intentionally conservative: runtime skills and external tools are not parallel-safe even when they declare read permission.

- [x] **Step 4: Run parallel policy tests to verify they pass**

Run:

```bash
pixi run cargo test -p agent-runtime tools::registry_tests::read_only_builtin_tools_are_parallel_safe -- --nocapture
pixi run cargo test -p agent-runtime tools::registry_tests::write_command_runtime_and_external_tools_are_not_parallel_safe_by_default -- --nocapture
```

- [x] **Step 5: Commit parallel policy**

Run:

```bash
git add crates/agent-runtime/src/tools/mod.rs crates/agent-runtime/src/tools/registry_tests.rs
git commit -m "feat: add conservative tool parallel policy"
```

## Task 7: Phase 7 Verification and Documentation

**Files:**
- Modify: `docs/mvp-verification.md`
- Modify: `docs/superpowers/plans/2026-06-28-codex-like-runtime-phase-7.md`

- [x] **Step 1: Run full verification**

Run:

```bash
pixi run cargo test --workspace
pixi run cargo clippy --workspace --all-targets -- -D warnings
pixi run cargo fmt --all --check
git diff --check HEAD
find crates apps scripts -type f \( -name '*.rs' -o -name '*.ts' -o -name '*.tsx' -o -name '*.js' -o -name '*.jsx' -o -name '*.css' -o -name '*.mjs' \) -not -path '*/target/*' -not -path '*/node_modules/*' -print0 | xargs -0 wc -l | sort -nr | head -20
```

- [x] **Step 2: Append verification record**

Record:

- Streaming tool-call argument assembly is test-covered without claiming real SSE support.
- Goal context and usage budgets are turn-local and evented.
- Context compaction is deterministic and preserves authority blocks.
- Subagent lifecycle is represented as structured runtime events through a service shell.
- Parallel-safe policy is conservative and excludes runtime skills/external tools by default.
- Real parallel scheduler, real subagents, real SSE, persistent goals, and plugin install remain later work.

- [x] **Step 3: Commit documentation**

Run:

```bash
git add docs/mvp-verification.md docs/superpowers/plans/2026-06-28-codex-like-runtime-phase-7.md
git commit -m "docs: record codex-like runtime phase 7 verification"
```

## Phase 7 Acceptance Checklist

- [x] Streaming tool-call arguments can be assembled before execution.
- [x] Parallel-safe policy exists and is conservative.
- [x] Runtime skills and external tools are not parallel-safe by default.
- [x] Turn requests can carry an active goal.
- [x] Active goal context is injected into model input.
- [x] Gateway usage events are reported as runtime events.
- [x] Token budget excess stops the turn with a structured failure.
- [x] Deterministic context compaction preserves authority blocks.
- [x] Subagent lifecycle shell emits started, finished, and failed events.
- [x] Documentation states real SSE, real subagents, real parallel scheduling, and plugin install remain later work.
- [x] No edited/new source file exceeds 1000 physical lines.
