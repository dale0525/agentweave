# Codex-Like Runtime Phase 5 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build Phase 5 of the Codex-like runtime: approval and sandbox policy foundations that can block consequential actions before execution and report approval-required outcomes through structured events.

**Architecture:** Add policy types to `agent-runtime` and store them in `RuntimeConfig`. `ToolRegistry` decides whether a tool permission requires approval before execution. `TurnRunner` checks this before emitting raw tool-call-start events, returns a structured `approval_required` tool result to the model, and emits a dedicated approval event without raw arguments.

**Tech Stack:** Rust 2024, serde/serde_json, existing `RuntimeConfig`, `ToolRegistry`, `ToolResult`, `RuntimeEvent`, pixi-managed cargo commands.

---

## Scope

Phase 5 scope from the migration design:

- Approval policy data structures.
- Approval workflow foundation for permission transitions.
- Stronger sandbox profile declarations.
- Network access policy placeholder.
- Structured events for approval-required outcomes.
- API-visible blocked action result that does not leak raw tool arguments.

Out of scope:

- Interactive approval UI.
- Persistent approval decision storage.
- Real OS sandboxing, Seatbelt, seccomp, or network isolation.
- Public end-user approval controls.
- MCP/connectors and deferred tools.

## File Structure

Create:

- `crates/agent-runtime/src/policy.rs`
  - Owns `ApprovalPolicy`, `SandboxProfile`, `FilesystemPolicy`, `CommandPolicy`, and `NetworkPolicy`.

Modify:

- `crates/agent-runtime/src/lib.rs`
  - Export `policy`.
- `crates/agent-runtime/src/tools/mod.rs`
  - Add policy fields to `RuntimeConfig`.
  - Add `ToolRegistry::approval_requirement(name)`.
- `crates/agent-runtime/src/events.rs`
  - Add `ApprovalRequired`.
- `crates/agent-runtime/src/turn.rs`
  - Check approval before emitting `ToolCallStarted`.
  - Return structured `approval_required` tool result and observation.
- `docs/mvp-verification.md`
  - Append Phase 5 verification evidence.

## Task 1: Policy Data Structures

**Files:**
- Create: `crates/agent-runtime/src/policy.rs`
- Modify: `crates/agent-runtime/src/lib.rs`
- Modify: `crates/agent-runtime/src/tools/mod.rs`

- [x] **Step 1: Write failing policy tests**

Add tests in `policy.rs`:

```rust
#[test]
fn approval_policy_identifies_permissions_that_require_approval() {
    assert!(!ApprovalPolicy::Never.requires_approval(ToolPermission::WriteWorkspace));
    assert!(ApprovalPolicy::OnWorkspaceWrite.requires_approval(ToolPermission::WriteWorkspace));
    assert!(ApprovalPolicy::OnWorkspaceWrite.requires_approval(ToolPermission::ExecuteCommand));
    assert!(!ApprovalPolicy::OnCommand.requires_approval(ToolPermission::WriteWorkspace));
    assert!(ApprovalPolicy::OnCommand.requires_approval(ToolPermission::ExecuteCommand));
}

#[test]
fn default_sandbox_profile_is_explicit_about_network_placeholder() {
    let profile = SandboxProfile::default();

    assert_eq!(profile.filesystem, FilesystemPolicy::WorkspaceOnly);
    assert_eq!(profile.command, CommandPolicy::DevelopmentOnly);
    assert_eq!(profile.network, NetworkPolicy::UnrestrictedPlaceholder);
}
```

Add a `RuntimeConfig` default test in `tools/mod.rs`:

```rust
assert_eq!(workspace_write.approval_policy, ApprovalPolicy::Never);
assert_eq!(workspace_write.sandbox_profile.network, NetworkPolicy::UnrestrictedPlaceholder);
```

- [x] **Step 2: Run policy tests to verify they fail**

Run:

```bash
pixi run cargo test -p agent-runtime policy::tests tools::tests::runtime_config_defaults_to_command_disabled -- --nocapture
```

Expected: fail because policy module and runtime config fields do not exist.

- [x] **Step 3: Implement policy types**

Add:

```rust
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum ApprovalPolicy {
    Never,
    OnWorkspaceWrite,
    OnCommand,
}
```

`OnWorkspaceWrite` requires approval for `WriteWorkspace` and `ExecuteCommand`. `OnCommand` requires approval only for `ExecuteCommand`.

Add:

```rust
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum FilesystemPolicy { WorkspaceOnly }

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum CommandPolicy { Disabled, DevelopmentOnly }

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum NetworkPolicy { UnrestrictedPlaceholder }

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub struct SandboxProfile {
    pub filesystem: FilesystemPolicy,
    pub command: CommandPolicy,
    pub network: NetworkPolicy,
}
```

Default sandbox profile is workspace-only filesystem, development-only command, unrestricted network placeholder.

- [x] **Step 4: Run policy tests to verify they pass**

Run:

```bash
pixi run cargo test -p agent-runtime policy::tests -- --nocapture
pixi run cargo test -p agent-runtime tools::tests::runtime_config_defaults_to_command_disabled -- --nocapture
```

- [x] **Step 5: Commit policy task**

Run:

```bash
git add crates/agent-runtime/src/policy.rs crates/agent-runtime/src/lib.rs crates/agent-runtime/src/tools/mod.rs
git commit -m "feat: add runtime approval and sandbox policy types"
```

## Task 2: Approval-Required Tool Blocking

**Files:**
- Modify: `crates/agent-runtime/src/tools/mod.rs`
- Modify: `crates/agent-runtime/src/events.rs`
- Modify: `crates/agent-runtime/src/turn.rs`

- [x] **Step 1: Write failing approval tests**

Add a tool registry test:

```rust
#[tokio::test]
async fn tool_registry_reports_approval_requirement_for_write_tools() {
    let root = unique_test_dir("approval-requirement");
    std::fs::create_dir_all(&root).unwrap();
    let mut config = RuntimeConfig::workspace_write(root.clone(), root.clone());
    config.approval_policy = ApprovalPolicy::OnWorkspaceWrite;
    let registry = ToolRegistry::new(SkillRegistry::empty_for_tests(), &config);

    let requirement = registry.approval_requirement("create_directory").unwrap();

    assert_eq!(requirement.permission, ToolPermission::WriteWorkspace);
    assert_eq!(requirement.policy, ApprovalPolicy::OnWorkspaceWrite);
    remove_test_dir(root).await;
}
```

Add a turn-loop test:

```rust
#[tokio::test]
async fn approval_required_blocks_tool_before_raw_arguments_event() {
    let workspace = test_workspace("approval-required");
    let skills = SkillRegistry::load(skills_root()).await.unwrap();
    let mut config = RuntimeConfig::workspace_write(workspace.clone(), workspace.clone());
    config.approval_policy = crate::policy::ApprovalPolicy::OnWorkspaceWrite;
    let runner = TurnRunner::new_with_config(
        FakePhaseTwoModel {
            calls: AtomicUsize::new(0),
            tool_name: "create_directory",
            arguments: serde_json::json!({ "path": "blocked-secret-path" }),
            requests: Mutex::new(Vec::new()),
        },
        skills,
        config,
    );

    let events = runner.run("create a directory").await.unwrap();

    assert!(!workspace.join("blocked-secret-path").exists());
    assert!(events.iter().any(|event| matches!(
        event,
        RuntimeEvent::ApprovalRequired { name, .. } if name == "create_directory"
    )));
    assert!(!events.iter().any(|event| matches!(
        event,
        RuntimeEvent::ToolCallStarted { arguments, .. }
            if arguments.to_string().contains("blocked-secret-path")
    )));
    let result = tool_result(&events);
    assert_eq!(result["ok"], false);
    assert_eq!(result["error"]["code"], "approval_required");
    remove_workspace(&workspace);
}
```

- [x] **Step 2: Run approval tests to verify they fail**

Run:

```bash
pixi run cargo test -p agent-runtime tools::tests::tool_registry_reports_approval_requirement_for_write_tools -- --nocapture
pixi run cargo test -p agent-runtime turn::tests::approval_required_blocks_tool_before_raw_arguments_event -- --nocapture
```

Expected: fail because approval requirement and event are missing.

- [x] **Step 3: Implement approval requirement and event**

Add:

```rust
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ApprovalRequirement {
    pub permission: ToolPermission,
    pub policy: ApprovalPolicy,
}
```

`ToolRegistry::approval_requirement(name)` returns `Some` when the tool exists and the configured policy requires approval for its permission.

Add event:

```rust
ApprovalRequired {
    call_id: String,
    name: String,
    permission: ToolPermission,
    policy: ApprovalPolicy,
}
```

In `TurnRunner`, when a model emits a tool call:

1. Check `self.tools.approval_requirement(&name)`.
2. If approval is required, emit `ApprovalRequired`.
3. Emit `ToolCallFinished` with a `ToolResult::failure` whose error code is `approval_required` and whose message is `Tool call requires approval before execution.`
4. Append the failure observation to model input.
5. Do not emit `ToolCallStarted` with raw arguments.

- [x] **Step 4: Run approval tests to verify they pass**

Run:

```bash
pixi run cargo test -p agent-runtime tools::tests::tool_registry_reports_approval_requirement_for_write_tools -- --nocapture
pixi run cargo test -p agent-runtime turn::tests::approval_required_blocks_tool_before_raw_arguments_event -- --nocapture
```

- [x] **Step 5: Commit approval blocking task**

Run:

```bash
git add crates/agent-runtime/src/tools/mod.rs crates/agent-runtime/src/events.rs crates/agent-runtime/src/turn.rs
git commit -m "feat: block approval-required tool calls"
```

## Task 3: Phase 5 Verification and Documentation

**Files:**
- Modify: `docs/mvp-verification.md`
- Modify: `docs/superpowers/plans/2026-06-28-codex-like-runtime-phase-5.md`

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

- Approval-required actions are blocked before execution.
- Approval decisions are represented in structured runtime events.
- Sandbox profiles explicitly state filesystem, command, and network behavior.
- Network policy is a placeholder and does not claim isolation.
- API-visible approval-required results do not include raw tool arguments.

- [x] **Step 3: Commit documentation**

Run:

```bash
git add docs/mvp-verification.md docs/superpowers/plans/2026-06-28-codex-like-runtime-phase-5.md
git commit -m "docs: record codex-like runtime phase 5 verification"
```

## Phase 5 Acceptance Checklist

- [x] Runtime has approval policy data structures.
- [x] Runtime has explicit sandbox profile and network policy placeholder.
- [x] Runtime can request approval without executing blocked actions.
- [x] Approval-required outcomes are represented in structured events.
- [x] Approval-required tool results use `approval_required`.
- [x] Blocked approval-required calls do not emit raw tool arguments in `ToolCallStarted`.
- [x] API can report approval-required result through runtime events/tool result.
- [x] Documentation states that real sandbox/network isolation remains later work.
- [x] `docs/mvp-verification.md` records Phase 5 verification evidence.
- [x] No edited/new source file exceeds 1000 physical lines.
