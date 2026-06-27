# Codex-Like Runtime Phase 4 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build Phase 4 of the Codex-like runtime: developer diagnostics for visible tools and instruction context, with richer tool metadata and schema validation, while keeping production APIs free of user-facing tool management.

**Architecture:** Extend runtime tool descriptors with source, namespace, optional output schema, and validation status. Add a separate `agent-server/src/dev_api.rs` module that is mounted only when explicitly requested. Reuse `InstructionContext` and `SkillCatalog` for `/dev/instructions/preview` so diagnostics explain exactly what the model would see.

**Tech Stack:** Rust 2024, Axum, serde/serde_json, existing `ToolRegistry`, `InstructionContext`, `SkillRegistry`, `SkillCatalog`, pixi-managed cargo commands.

---

## Scope

Phase 4 scope from the migration design:

- Tool registry inspection for dev mode.
- Dev-only `/dev/tools`.
- Dev-only `/dev/instructions/preview`.
- Tool schema validation improvements.
- Tool result output schema checks where declared.
- Persistent runtime diagnostics remain out of scope for this phase.

Out of scope:

- Public end-user skill or tool UI.
- Persistent runtime event storage.
- Approval workflow and sandbox policy; those are Phase 5.
- MCP, connectors, and deferred tools; those are Phase 6.
- Streaming and parallel tool calls; those are Phase 7.

## Current Context

- Work on `main`; do not create a worktree.
- `.codex/` is untracked and must remain untracked unless explicitly requested.
- `crates/agent-server/src/api.rs` is around 943 lines. Do not add dev endpoint handlers there.
- `crates/agent-runtime/src/tools/builtin.rs` is around 943 lines. Avoid adding bulky tests or endpoint code there.
- `ToolDefinition` currently contains only `name`, `description`, `input_schema`, and `permission`.
- Runtime `skill.json` manifests now validate tool names and `input_schema.type == "object"`.
- Phase 3 added `SkillCatalog`, available skill summaries, and triggered full `SKILL.md` injection.

## File Structure

Create:

- `crates/agent-runtime/src/tools/schema.rs`
  - Owns reusable tool schema validation and diagnostic structs.
- `crates/agent-server/src/dev_api.rs`
  - Owns dev-only routes and response types for `/dev/tools` and `/dev/instructions/preview`.

Modify:

- `crates/agent-runtime/src/tools/mod.rs`
  - Export `schema`.
  - Add `ToolSource`.
  - Add `namespace: Option<String>`, `output_schema: Option<Value>`, and `source: ToolSource` to `ToolDefinition`.
  - Add `ToolRegistry::diagnostics()`.
- `crates/agent-runtime/src/tools/builtin.rs`
  - Mark built-in definitions with `ToolSource::BuiltIn` and no output schema.
- `crates/agent-runtime/src/skill.rs`
  - Add `SkillRegistry::tool_definitions()` or enough metadata for `ToolRegistry` to report `ToolSource::RuntimeSkill { skill_name }`.
- `crates/agent-server/src/api.rs`
  - Expose safe `AppState` accessors needed by `dev_api.rs`.
  - Keep production router behavior unchanged by default.
- `crates/agent-server/src/main.rs`
  - Mount dev routes only when `GENERAL_AGENT_DEV_API=1`.
- `docs/mvp-verification.md`
  - Append Phase 4 verification evidence.

No desktop UI files should change in Phase 4.

## Task 1: Tool Metadata and Schema Diagnostics

**Files:**
- Create: `crates/agent-runtime/src/tools/schema.rs`
- Modify: `crates/agent-runtime/src/tools/mod.rs`
- Modify: `crates/agent-runtime/src/tools/builtin.rs`
- Modify: `crates/agent-runtime/src/skill.rs`

- [ ] **Step 1: Write failing tool metadata tests**

Add tests in `crates/agent-runtime/src/tools/mod.rs`:

```rust
#[test]
fn tool_definitions_include_source_and_schema_diagnostics() {
    let workspace_root = PathBuf::from("/workspace");
    let config = RuntimeConfig::workspace_write(workspace_root.clone(), workspace_root);
    let registry = ToolRegistry::new(SkillRegistry::empty_for_tests(), &config);

    let diagnostics = registry.diagnostics();
    let create_directory = diagnostics
        .iter()
        .find(|tool| tool.name == "create_directory")
        .expect("create_directory diagnostic should exist");

    assert_eq!(create_directory.source, ToolSource::BuiltIn);
    assert_eq!(create_directory.permission, ToolPermission::WriteWorkspace);
    assert!(create_directory.schema.valid);
    assert_eq!(create_directory.namespace, None);
}
```

Add a test that a runtime skill diagnostic reports runtime source:

```rust
#[tokio::test]
async fn runtime_skill_diagnostics_include_skill_source() {
    let root = unique_test_dir("runtime-source-diagnostics");
    write_skill(
        &root,
        "echoer",
        "echoer_echo",
        "process.stdin.resume();\nprocess.stdin.on('data', (chunk) => process.stdout.write(chunk));\n",
    )
    .await;
    let skills = SkillRegistry::load_development(&root).await.unwrap();
    let config = RuntimeConfig::workspace_write(root.clone(), root.clone());
    let registry = ToolRegistry::new(skills, &config);

    let diagnostic = registry
        .diagnostics()
        .into_iter()
        .find(|tool| tool.name == "echoer_echo")
        .unwrap();

    assert_eq!(
        diagnostic.source,
        ToolSource::RuntimeSkill {
            skill_name: "echoer".into()
        }
    );
    assert!(diagnostic.schema.valid);
    remove_test_dir(root).await;
}
```

- [ ] **Step 2: Run metadata tests to verify they fail**

Run:

```bash
pixi run cargo test -p agent-runtime tools::tests::tool_definitions_include_source_and_schema_diagnostics tools::tests::runtime_skill_diagnostics_include_skill_source -- --nocapture
```

Expected: fail because `ToolSource`, diagnostics, and test helpers are missing.

- [ ] **Step 3: Implement metadata and schema diagnostics**

Add to `tools/mod.rs`:

```rust
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub enum ToolSource {
    BuiltIn,
    RuntimeSkill { skill_name: String },
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub namespace: Option<String>,
    pub description: String,
    pub input_schema: Value,
    pub output_schema: Option<Value>,
    pub permission: ToolPermission,
    pub source: ToolSource,
}
```

Add `ToolSchemaDiagnostic` and `ToolDiagnostic` in `tools/schema.rs`, plus `validate_tool_definition(&ToolDefinition) -> ToolSchemaDiagnostic`. The MVP validation rules:

- `name` is non-empty, at most 64 chars, and contains only ASCII alphanumeric, `_`, or `-`.
- `namespace`, when present, follows the same character rule.
- `input_schema.type` must be `"object"`.
- `output_schema`, when present, must be a JSON object.

Add `ToolRegistry::diagnostics() -> Vec<ToolDiagnostic>`, sorted by `(namespace, name)`.

- [ ] **Step 4: Run metadata tests to verify they pass**

Run:

```bash
pixi run cargo test -p agent-runtime tools::tests::tool_definitions_include_source_and_schema_diagnostics -- --nocapture
pixi run cargo test -p agent-runtime tools::tests::runtime_skill_diagnostics_include_skill_source -- --nocapture
```

Expected: both tests pass.

- [ ] **Step 5: Commit metadata task**

Run:

```bash
git add crates/agent-runtime/src/tools/mod.rs crates/agent-runtime/src/tools/schema.rs crates/agent-runtime/src/tools/builtin.rs crates/agent-runtime/src/skill.rs
git commit -m "feat: add runtime tool diagnostics metadata"
```

## Task 2: Dev-Only `/dev/tools`

**Files:**
- Create: `crates/agent-server/src/dev_api.rs`
- Modify: `crates/agent-server/src/api.rs`
- Modify: `crates/agent-server/src/main.rs`

- [ ] **Step 1: Write failing server tests**

Add tests in `dev_api.rs`:

```rust
#[tokio::test]
async fn dev_tools_route_is_not_mounted_by_default() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let app = crate::api::router(Arc::new(crate::api::AppState::new(storage)));

    let response = app
        .oneshot(Request::builder().uri("/dev/tools").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn dev_tools_route_returns_tool_diagnostics_when_enabled() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let skills = development_skills().await;
    let state = Arc::new(crate::api::AppState::new_with_agent_and_skills(
        storage,
        Arc::new(crate::api::DeterministicAgent),
        skills,
    ));
    let app = crate::api::router_with_dev_routes(state);

    let response = app
        .oneshot(Request::builder().uri("/dev/tools").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = read_json(response).await;
    assert!(body["tools"].as_array().unwrap().iter().any(|tool| tool["name"] == "create_directory"));
    assert!(body["tools"].as_array().unwrap().iter().all(|tool| tool.get("schema").is_some()));
}
```

- [ ] **Step 2: Run server tests to verify they fail**

Run:

```bash
pixi run cargo test -p agent-server dev_api::tests -- --nocapture
```

Expected: fail because `dev_api` and `router_with_dev_routes` do not exist.

- [ ] **Step 3: Implement `dev_api.rs` and router mounting**

Add `pub mod dev_api;` in `main.rs` and `#[cfg(test)] mod dev_api;` support as needed for tests.

Expose:

```rust
pub fn router_with_dev_routes(state: Arc<AppState>) -> Router
```

This wraps the normal production routes and merges `dev_api::router(state.clone())`.

`/dev/tools` returns:

```json
{
  "tools": [
    {
      "name": "create_directory",
      "namespace": null,
      "description": "Create a directory inside the workspace.",
      "permission": "WriteWorkspace",
      "source": { "BuiltIn": null },
      "schema": { "valid": true, "errors": [] }
    }
  ]
}
```

Use `RuntimeConfig` plus `SkillRegistry` from `AppState` to build a `ToolRegistry` and call `diagnostics()`.

- [ ] **Step 4: Wire env-gated startup**

In `main.rs`, choose:

```rust
let router = if std::env::var("GENERAL_AGENT_DEV_API").as_deref() == Ok("1") {
    api::router_with_dev_routes(state)
} else {
    api::router(state)
};
```

- [ ] **Step 5: Run dev tools tests**

Run:

```bash
pixi run cargo test -p agent-server dev_api::tests -- --nocapture
pixi run cargo test -p agent-server api::tests::production_router_does_not_expose_skill_inventory -- --nocapture
```

Expected: dev route exists only through explicit dev router; production router stays closed.

- [ ] **Step 6: Commit dev tools task**

Run:

```bash
git add crates/agent-server/src/dev_api.rs crates/agent-server/src/api.rs crates/agent-server/src/main.rs
git commit -m "feat: add dev tool diagnostics endpoint"
```

## Task 3: Dev-Only `/dev/instructions/preview`

**Files:**
- Modify: `crates/agent-server/src/dev_api.rs`
- Modify: `crates/agent-server/src/api.rs`

- [ ] **Step 1: Write failing preview tests**

Add a dev API test:

```rust
#[tokio::test]
async fn dev_instructions_preview_returns_project_and_skill_context() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let workspace = unique_test_dir("instructions-preview");
    tokio::fs::write(workspace.join("AGENTS.md"), "Project rules").await.unwrap();
    let skills_root = workspace.join("skills");
    tokio::fs::create_dir_all(skills_root.join("planning")).await.unwrap();
    tokio::fs::write(
        skills_root.join("planning").join("SKILL.md"),
        "---\nname: planning\ndescription: Write plans.\n---\n\n# Planning\nUse checklists.",
    )
    .await
    .unwrap();
    let skills = SkillRegistry::load_development(&skills_root).await.unwrap();
    let catalog = SkillCatalog::load_development(&skills_root).await.unwrap();
    let state = Arc::new(
        crate::api::AppState::new_with_agent_and_skills(storage, Arc::new(crate::api::DeterministicAgent), skills)
            .with_runtime_config(RuntimeConfig::workspace_write(workspace.clone(), workspace.clone()))
            .with_skill_catalog(catalog),
    );
    let app = crate::api::router_with_dev_routes(state);

    let response = app
        .oneshot(json_request(
            "/dev/instructions/preview",
            json!({ "content": "use $planning" }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = read_json(response).await;
    assert!(body["developer"].as_str().unwrap().contains("Project rules"));
    assert!(body["developer"].as_str().unwrap().contains("<available_skills"));
    assert!(body["developer"].as_str().unwrap().contains("<skill_instructions name=\"planning\""));
    remove_test_dir(workspace).await;
}
```

- [ ] **Step 2: Run preview test to verify it fails**

Run:

```bash
pixi run cargo test -p agent-server dev_api::tests::dev_instructions_preview_returns_project_and_skill_context -- --nocapture
```

Expected: fail because preview route and `AppState` catalog accessors do not exist.

- [ ] **Step 3: Implement preview route**

`POST /dev/instructions/preview` accepts:

```rust
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstructionsPreviewRequest {
    pub content: String,
}
```

It builds `InstructionConfig` from `state.runtime_config`, injects `state.skill_catalog.summaries()`, loads triggered full docs using the request content, and returns:

```rust
#[derive(Debug, Serialize)]
pub struct InstructionsPreviewResponse {
    pub system: String,
    pub developer: String,
    pub user: String,
    pub triggered_skills: Vec<String>,
}
```

Extract these strings from `InstructionContext::model_input`.

- [ ] **Step 4: Run preview tests**

Run:

```bash
pixi run cargo test -p agent-server dev_api::tests::dev_instructions_preview_returns_project_and_skill_context -- --nocapture
pixi run cargo test -p agent-server dev_api::tests -- --nocapture
```

Expected: dev preview and dev tools tests pass.

- [ ] **Step 5: Commit preview task**

Run:

```bash
git add crates/agent-server/src/dev_api.rs crates/agent-server/src/api.rs
git commit -m "feat: add dev instruction preview endpoint"
```

## Task 4: Phase 4 Verification and Documentation

**Files:**
- Modify: `docs/mvp-verification.md`
- Modify: `docs/superpowers/plans/2026-06-28-codex-like-runtime-phase-4.md`

- [ ] **Step 1: Run full verification**

Run:

```bash
pixi run cargo test --workspace
pixi run cargo clippy --workspace --all-targets -- -D warnings
pixi run cargo fmt --all --check
git diff --check HEAD
find crates apps scripts -type f \( -name '*.rs' -o -name '*.ts' -o -name '*.tsx' -o -name '*.js' -o -name '*.jsx' -o -name '*.css' -o -name '*.mjs' \) -not -path '*/target/*' -not -path '*/node_modules/*' -print0 | xargs -0 wc -l | sort -nr | head -20
```

Expected: all checks pass and no edited/new source file exceeds 1000 lines.

- [ ] **Step 2: Append verification record**

Append a `Codex-Like Runtime Phase 4 Verification` section to `docs/mvp-verification.md` with exact command results and notes:

- `/dev/tools` is mounted only through dev router or `GENERAL_AGENT_DEV_API=1`.
- `/dev/instructions/preview` returns deterministic system/developer/user blocks.
- Production router remains free of skill/tool management endpoints.
- Persistent runtime diagnostics are intentionally deferred.

- [ ] **Step 3: Commit documentation**

Run:

```bash
git add docs/mvp-verification.md docs/superpowers/plans/2026-06-28-codex-like-runtime-phase-4.md
git commit -m "docs: record codex-like runtime phase 4 verification"
```

## Phase 4 Acceptance Checklist

- [ ] Tool definitions include source metadata.
- [ ] Tool definitions include namespace and optional output schema fields.
- [ ] Tool schema diagnostics validate names and schema shapes.
- [ ] Runtime skill tools report `RuntimeSkill` source metadata.
- [ ] Development diagnostics can explain visible tool metadata.
- [ ] Production API does not expose dev diagnostics by default.
- [ ] `/dev/tools` is available only through explicit dev mode.
- [ ] `/dev/instructions/preview` explains model-visible instruction context.
- [ ] Invalid tool schemas fail fast during runtime skill loading or are reported invalid in diagnostics.
- [ ] Persistent runtime diagnostics are documented as deferred.
- [ ] `docs/mvp-verification.md` records Phase 4 verification evidence.
- [ ] No edited/new source file exceeds 1000 physical lines.
