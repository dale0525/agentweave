# Codex-Like Runtime Phase 6 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build Phase 6 of the Codex-like runtime: MCP/connector metadata foundations, model-safe namespaced tool discovery, and deferred tool advertisement that does not load every full schema into every turn.

**Architecture:** Keep the existing `ToolRegistry` execution path for built-ins and runtime skills, then add a discovery layer for external MCP-style tools and connector metadata. Immediate external tools are flattened into model-safe names such as `mcp__server__tool`; deferred external tools are visible through diagnostics/discovery summaries but are excluded from `TurnRunner` model tool schemas until a later materialization phase.

**Tech Stack:** Rust 2024, serde/serde_json, existing `RuntimeConfig`, `ToolRegistry`, `ToolDefinition`, dev-only Axum routes, pixi-managed cargo commands.

---

## Scope

Phase 6 scope from the migration design:

- MCP-style tool adapter metadata model.
- Namespaced model-safe tool names.
- Collision checks between built-ins, runtime skills, and external tools.
- Deferred tool discovery summaries.
- Connector metadata model.
- Dev-only discovery endpoint for visible tools, deferred tools, and connector metadata.

Out of scope:

- Real MCP server process management.
- Real connector authentication or installation.
- Materializing deferred schemas on demand inside a turn.
- Executing real external MCP or connector processes.
- Model-visible `discover_tools` / `load_deferred_tool` activation tools.
- User-facing desktop tool, skill, marketplace, or connector UI.

## File Structure

Create:

- `crates/agent-runtime/src/tools/discovery.rs`
  - Owns external tool configuration, connector metadata, deferred discovery summaries, and model-safe namespaced name helpers.

Modify:

- `crates/agent-runtime/src/tools/mod.rs`
  - Export `discovery`.
  - Extend `RuntimeConfig` with external tools and connectors.
  - Extend `ToolSource` with `Mcp` and `AppConnector`.
  - Add registry collision validation and `ToolRegistry::discovery()`.
- `crates/agent-runtime/src/turn.rs`
  - Keep deferred tools out of model-visible `GatewayTool` schemas.
- `crates/agent-server/src/dev_api.rs`
  - Add `GET /dev/tool-discovery`.
- `docs/mvp-verification.md`
  - Append Phase 6 verification evidence.

## Task 1: Discovery Types and Namespacing

**Files:**
- Create: `crates/agent-runtime/src/tools/discovery.rs`
- Modify: `crates/agent-runtime/src/tools/mod.rs`

- [ ] **Step 1: Write failing discovery type tests**

Add tests in `tools/discovery.rs`:

```rust
#[test]
fn mcp_tool_names_are_flattened_with_model_safe_namespace() {
    let tool = ExternalToolConfig::mcp(
        "filesystem",
        "read_file",
        "Read a file through MCP.",
        serde_json::json!({ "type": "object" }),
        ExternalToolVisibility::Immediate,
    );

    assert_eq!(tool.flattened_name().unwrap(), "mcp__filesystem__read_file");
    assert_eq!(tool.namespace().unwrap(), "mcp__filesystem");
}

#[test]
fn external_tool_names_reject_invalid_namespace_parts() {
    let tool = ExternalToolConfig::mcp(
        "bad/server",
        "read_file",
        "Read a file through MCP.",
        serde_json::json!({ "type": "object" }),
        ExternalToolVisibility::Immediate,
    );

    let error = tool.flattened_name().unwrap_err().to_string();

    assert!(error.contains("external tool namespace must be model-safe"));
}

#[test]
fn deferred_external_tool_summaries_do_not_require_full_schema() {
    let tool = ExternalToolConfig::mcp(
        "search",
        "expensive_lookup",
        "Search a remote corpus.",
        serde_json::json!({ "type": "object", "properties": { "query": { "type": "string" } } }),
        ExternalToolVisibility::Deferred {
            summary: "Remote corpus lookup.".into(),
        },
    );

    let summary = tool.discovery_summary().unwrap();

    assert_eq!(summary.name, "mcp__search__expensive_lookup");
    assert_eq!(summary.namespace.as_deref(), Some("mcp__search"));
    assert!(!summary.schema_loaded);
    assert_eq!(summary.summary, "Remote corpus lookup.");
}
```

- [ ] **Step 2: Run discovery tests to verify they fail**

Run:

```bash
pixi run cargo test -p agent-runtime tools::discovery::tests -- --nocapture
```

Expected: fail because the discovery module and types do not exist.

- [ ] **Step 3: Implement discovery types**

Add:

```rust
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorAuthState {
    NotRequired,
    Connected,
    Missing,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ConnectorMetadata {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub permissions: Vec<ToolPermission>,
    pub auth_state: ConnectorAuthState,
    pub tool_count: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub enum ExternalToolVisibility {
    Immediate,
    Deferred { summary: String },
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub enum ExternalToolExecution {
    Unavailable,
    Static { result: serde_json::Value },
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub enum ExternalToolKind {
    Mcp { server: String },
    AppConnector { connector: String },
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ExternalToolConfig {
    pub kind: ExternalToolKind,
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub permission: ToolPermission,
    pub visibility: ExternalToolVisibility,
    pub execution: ExternalToolExecution,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ToolDiscoveryItem {
    pub name: String,
    pub namespace: Option<String>,
    pub description: String,
    pub summary: String,
    pub permission: ToolPermission,
    pub source: ToolSource,
    pub schema_loaded: bool,
    pub deferred: bool,
}
```

Implement:

```rust
impl ExternalToolConfig {
    pub fn mcp(
        server: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: serde_json::Value,
        visibility: ExternalToolVisibility,
    ) -> Self { /* set permission to ToolPermission::ReadWorkspace for MVP */ }

    pub fn with_static_result(mut self, result: serde_json::Value) -> Self { /* test/static adapter */ }
    pub fn namespace(&self) -> anyhow::Result<String> { /* mcp__server or connector__id */ }
    pub fn flattened_name(&self) -> anyhow::Result<String> { /* namespace__name */ }
    pub fn tool_definition(&self) -> anyhow::Result<Option<ToolDefinition>> { /* None for deferred */ }
    pub fn discovery_summary(&self) -> anyhow::Result<ToolDiscoveryItem> { /* schema_loaded false for deferred */ }
}
```

The name helper must accept only ASCII alphanumeric, `_`, or `-` in each part and return an error for invalid namespace or tool names.

- [ ] **Step 4: Run discovery tests to verify they pass**

Run:

```bash
pixi run cargo test -p agent-runtime tools::discovery::tests -- --nocapture
```

- [ ] **Step 5: Commit discovery types**

Run:

```bash
git add crates/agent-runtime/src/tools/discovery.rs crates/agent-runtime/src/tools/mod.rs
git commit -m "feat: add external tool discovery metadata"
```

## Task 2: Registry Integration and Collision Checks

**Files:**
- Modify: `crates/agent-runtime/src/tools/mod.rs`

- [ ] **Step 1: Write failing registry tests**

Add tests in `tools/mod.rs`:

```rust
#[tokio::test]
async fn tool_registry_includes_immediate_mcp_tools_with_namespaced_names() {
    let root = unique_test_dir("mcp-immediate");
    std::fs::create_dir_all(&root).unwrap();
    let config = RuntimeConfig {
        external_tools: vec![crate::tools::discovery::ExternalToolConfig::mcp(
            "filesystem",
            "read_file",
            "Read a file through MCP.",
            serde_json::json!({ "type": "object" }),
            crate::tools::discovery::ExternalToolVisibility::Immediate,
        )],
        ..RuntimeConfig::workspace_write(root.clone(), root.clone())
    };
    let registry = ToolRegistry::new(SkillRegistry::empty_for_tests(), &config);

    let tool = registry
        .definitions()
        .into_iter()
        .find(|tool| tool.name == "mcp__filesystem__read_file")
        .unwrap();

    assert_eq!(tool.namespace.as_deref(), Some("mcp__filesystem"));
    assert_eq!(tool.source, ToolSource::Mcp { server: "filesystem".into() });
    remove_test_dir(root).await;
}

#[tokio::test]
async fn tool_registry_executes_static_mcp_adapter_result() {
    let root = unique_test_dir("mcp-static-exec");
    std::fs::create_dir_all(&root).unwrap();
    let config = RuntimeConfig {
        external_tools: vec![crate::tools::discovery::ExternalToolConfig::mcp(
            "clock",
            "now",
            "Return a static time.",
            serde_json::json!({ "type": "object" }),
            crate::tools::discovery::ExternalToolVisibility::Immediate,
        )
        .with_static_result(serde_json::json!({ "time": "12:00" }))],
        ..RuntimeConfig::workspace_write(root.clone(), root.clone())
    };
    let registry = ToolRegistry::new(SkillRegistry::empty_for_tests(), &config);

    let result = registry
        .execute("mcp__clock__now", "call-1", serde_json::json!({}))
        .await;

    assert!(result.ok);
    assert_eq!(result.data.unwrap()["time"], "12:00");
    remove_test_dir(root).await;
}

#[tokio::test]
async fn tool_registry_rejects_namespaced_collisions() {
    let root = unique_test_dir("mcp-collision");
    std::fs::create_dir_all(&root).unwrap();
    let config = RuntimeConfig {
        external_tools: vec![
            crate::tools::discovery::ExternalToolConfig::mcp(
                "search",
                "lookup",
                "First lookup tool.",
                serde_json::json!({ "type": "object" }),
                crate::tools::discovery::ExternalToolVisibility::Immediate,
            ),
            crate::tools::discovery::ExternalToolConfig::mcp(
                "search",
                "lookup",
                "Second lookup tool.",
                serde_json::json!({ "type": "object" }),
                crate::tools::discovery::ExternalToolVisibility::Immediate,
            ),
        ],
        ..RuntimeConfig::workspace_write(root.clone(), root.clone())
    };

    let result = ToolRegistry::try_new(SkillRegistry::empty_for_tests(), &config);

    assert!(result.unwrap_err().to_string().contains("duplicate tool name"));
    remove_test_dir(root).await;
}

#[tokio::test]
async fn deferred_mcp_tools_are_discoverable_but_not_model_visible() {
    let root = unique_test_dir("mcp-deferred");
    std::fs::create_dir_all(&root).unwrap();
    let config = RuntimeConfig {
        external_tools: vec![crate::tools::discovery::ExternalToolConfig::mcp(
            "search",
            "expensive_lookup",
            "Search a remote corpus.",
            serde_json::json!({ "type": "object" }),
            crate::tools::discovery::ExternalToolVisibility::Deferred {
                summary: "Remote corpus lookup.".into(),
            },
        )],
        ..RuntimeConfig::workspace_write(root.clone(), root.clone())
    };
    let registry = ToolRegistry::new(SkillRegistry::empty_for_tests(), &config);

    assert!(!registry
        .definitions()
        .iter()
        .any(|tool| tool.name == "mcp__search__expensive_lookup"));
    assert!(registry
        .discovery()
        .tools
        .iter()
        .any(|tool| tool.name == "mcp__search__expensive_lookup" && tool.deferred));
    remove_test_dir(root).await;
}
```

Adjust helper cleanup to use an async test if needed; keep tests deterministic and do not rely on real MCP processes.

- [ ] **Step 2: Run registry tests to verify they fail**

Run:

```bash
pixi run cargo test -p agent-runtime tools::tests::tool_registry_includes_immediate_mcp_tools_with_namespaced_names -- --nocapture
pixi run cargo test -p agent-runtime tools::tests::tool_registry_executes_static_mcp_adapter_result -- --nocapture
pixi run cargo test -p agent-runtime tools::tests::tool_registry_rejects_namespaced_collisions -- --nocapture
pixi run cargo test -p agent-runtime tools::tests::deferred_mcp_tools_are_discoverable_but_not_model_visible -- --nocapture
```

Expected: fail because runtime config and registry discovery integration do not exist.

- [ ] **Step 3: Implement registry integration**

Update:

```rust
pub struct RuntimeConfig {
    /* existing fields */
    pub external_tools: Vec<discovery::ExternalToolConfig>,
    pub connectors: Vec<discovery::ConnectorMetadata>,
}
```

Default both fields to empty in `RuntimeConfig::new`.

Extend:

```rust
pub enum ToolSource {
    BuiltIn,
    RuntimeSkill { skill_name: String },
    Mcp { server: String },
    AppConnector { connector: String },
}
```

Add:

```rust
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ToolDiscovery {
    pub tools: Vec<discovery::ToolDiscoveryItem>,
    pub connectors: Vec<discovery::ConnectorMetadata>,
}

impl ToolRegistry {
    pub fn try_new(skills: SkillRegistry, config: &RuntimeConfig) -> anyhow::Result<Self> { /* validate */ }
    pub fn discovery(&self) -> ToolDiscovery { /* immediate definitions plus deferred summaries */ }
}
```

Keep `ToolRegistry::new` for existing callers:

```rust
pub fn new(skills: SkillRegistry, config: &RuntimeConfig) -> Self {
    Self::try_new(skills, config).expect("runtime tool registry should be valid")
}
```

Registry validation must reject duplicate final `ToolDefinition.name` values across built-ins, runtime skills, and immediate external tools.

`ToolRegistry::execute` must route immediate external tools with `ExternalToolExecution::Static` to a successful `ToolResult`. Immediate external tools with `ExternalToolExecution::Unavailable` must return `tool_disabled` with the message `External tool execution is not implemented in this phase.`

- [ ] **Step 4: Run registry tests to verify they pass**

Run:

```bash
pixi run cargo test -p agent-runtime tools::tests::tool_registry_includes_immediate_mcp_tools_with_namespaced_names -- --nocapture
pixi run cargo test -p agent-runtime tools::tests::tool_registry_executes_static_mcp_adapter_result -- --nocapture
pixi run cargo test -p agent-runtime tools::tests::tool_registry_rejects_namespaced_collisions -- --nocapture
pixi run cargo test -p agent-runtime tools::tests::deferred_mcp_tools_are_discoverable_but_not_model_visible -- --nocapture
```

- [ ] **Step 5: Commit registry integration**

Run:

```bash
git add crates/agent-runtime/src/tools/discovery.rs crates/agent-runtime/src/tools/mod.rs
git commit -m "feat: integrate deferred external tools into registry"
```

## Task 3: Turn and Dev Discovery Endpoint

**Files:**
- Modify: `crates/agent-runtime/src/turn.rs`
- Modify: `crates/agent-server/src/dev_api.rs`

- [ ] **Step 1: Write failing turn and API tests**

Add a turn-loop test in `turn.rs`:

```rust
#[tokio::test]
async fn deferred_external_tools_are_not_sent_as_model_tool_schemas() {
    let workspace = test_workspace("deferred-tools-hidden");
    let skills = SkillRegistry::load(skills_root()).await.unwrap();
    let config = RuntimeConfig {
        external_tools: vec![crate::tools::discovery::ExternalToolConfig::mcp(
            "search",
            "expensive_lookup",
            "Search a remote corpus.",
            serde_json::json!({ "type": "object" }),
            crate::tools::discovery::ExternalToolVisibility::Deferred {
                summary: "Remote corpus lookup.".into(),
            },
        )],
        ..RuntimeConfig::workspace_write(workspace.clone(), workspace.clone())
    };
    let runner = TurnRunner::new_with_config(
        ScriptedModel {
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            responses: vec![vec![
                GatewayEvent::TextDelta { text: "done".into() },
                GatewayEvent::Completed,
            ]],
        },
        skills,
        config,
    );

    let _events = runner.run("hello").await.unwrap();
    let requests = runner.model.requests.lock().unwrap();

    assert!(!request_has_tool(&requests[0], "mcp__search__expensive_lookup"));
    remove_workspace(&workspace);
}
```

Add a dev API test in `dev_api.rs`:

```rust
#[tokio::test]
async fn dev_tool_discovery_returns_deferred_tools_and_connectors() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let workspace = unique_test_dir("tool-discovery");
    tokio::fs::create_dir_all(&workspace).await.unwrap();
    let skills_root = workspace.join("skills");
    tokio::fs::create_dir_all(&skills_root).await.unwrap();
    let skills = SkillRegistry::load_development(&skills_root).await.unwrap();
    let mut config = RuntimeConfig::workspace_write(workspace.clone(), workspace.clone());
    config.external_tools.push(agent_runtime::tools::discovery::ExternalToolConfig::mcp(
        "search",
        "expensive_lookup",
        "Search a remote corpus.",
        serde_json::json!({ "type": "object" }),
        agent_runtime::tools::discovery::ExternalToolVisibility::Deferred {
            summary: "Remote corpus lookup.".into(),
        },
    ));
    config.connectors.push(agent_runtime::tools::discovery::ConnectorMetadata {
        id: "search".into(),
        name: "Search MCP".into(),
        description: "Remote search connector.".into(),
        version: "0.1.0".into(),
        permissions: vec![agent_runtime::tools::ToolPermission::ReadWorkspace],
        auth_state: agent_runtime::tools::discovery::ConnectorAuthState::NotRequired,
        tool_count: 1,
    });
    let state = Arc::new(
        crate::api::AppState::new_with_agent_and_skills(
            storage,
            Arc::new(TestAgent),
            skills,
        )
        .with_runtime_config(config),
    );
    let app = crate::api::router_with_dev_routes(state);

    let response = app
        .oneshot(Request::builder().uri("/dev/tool-discovery").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = read_json(response).await;
    assert!(body["tools"].as_array().unwrap().iter().any(|tool| {
        tool["name"] == "mcp__search__expensive_lookup"
            && tool["deferred"] == true
            && tool["schema_loaded"] == false
    }));
    assert_eq!(body["connectors"][0]["id"], "search");
    remove_test_dir(workspace).await;
}
```

- [ ] **Step 2: Run turn and API tests to verify they fail**

Run:

```bash
pixi run cargo test -p agent-runtime turn::tests::deferred_external_tools_are_not_sent_as_model_tool_schemas -- --nocapture
pixi run cargo test -p agent-server dev_api::tests::dev_tool_discovery_returns_deferred_tools_and_connectors -- --nocapture
```

Expected: fail because dev discovery route and config-driven deferred discovery are not wired.

- [ ] **Step 3: Implement turn and dev endpoint wiring**

Keep `TurnRunner` using `self.tools.definitions()` for model-visible tools. Because deferred tools return `None` from `ExternalToolConfig::tool_definition`, they remain out of every model request until future materialization.

Add to `dev_api.rs`:

```rust
#[derive(Debug, Serialize)]
struct DevToolDiscoveryResponse {
    tools: Vec<agent_runtime::tools::discovery::ToolDiscoveryItem>,
    connectors: Vec<agent_runtime::tools::discovery::ConnectorMetadata>,
}

async fn discover_tools(
    State(state): State<Arc<AppState>>,
) -> Result<Json<DevToolDiscoveryResponse>, StatusCode> {
    let skills = state.skills().ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let registry = ToolRegistry::new(skills, &state.runtime_config());
    let discovery = registry.discovery();

    Ok(Json(DevToolDiscoveryResponse {
        tools: discovery.tools,
        connectors: discovery.connectors,
    }))
}
```

Mount:

```rust
.route("/dev/tool-discovery", get(discover_tools))
```

- [ ] **Step 4: Run turn and API tests to verify they pass**

Run:

```bash
pixi run cargo test -p agent-runtime turn::tests::deferred_external_tools_are_not_sent_as_model_tool_schemas -- --nocapture
pixi run cargo test -p agent-server dev_api::tests::dev_tool_discovery_returns_deferred_tools_and_connectors -- --nocapture
```

- [ ] **Step 5: Commit turn/API discovery**

Run:

```bash
git add crates/agent-runtime/src/turn.rs crates/agent-server/src/dev_api.rs
git commit -m "feat: expose deferred tool discovery diagnostics"
```

## Task 4: Phase 6 Verification and Documentation

**Files:**
- Modify: `docs/mvp-verification.md`
- Modify: `docs/superpowers/plans/2026-06-28-codex-like-runtime-phase-6.md`

- [ ] **Step 1: Run full verification**

Run:

```bash
pixi run cargo test --workspace
pixi run cargo clippy --workspace --all-targets -- -D warnings
pixi run cargo fmt --all --check
git diff --check HEAD
find crates apps scripts -type f \( -name '*.rs' -o -name '*.ts' -o -name '*.tsx' -o -name '*.js' -o -name '*.jsx' -o -name '*.css' -o -name '*.mjs' \) -not -path '*/target/*' -not -path '*/node_modules/*' -print0 | xargs -0 wc -l | sort -nr | head -20
```

- [ ] **Step 2: Append verification record**

Record:

- Built-in tools, runtime skills, and fake MCP-style external tools share registry discovery.
- External tools use model-safe namespaced names and collision checks.
- Deferred external tools are visible in dev discovery but not sent as model tool schemas.
- Connector metadata is available through dev-only discovery.
- Real MCP execution, auth, install, and schema materialization remain later work.

- [ ] **Step 3: Commit documentation**

Run:

```bash
git add docs/mvp-verification.md docs/superpowers/plans/2026-06-28-codex-like-runtime-phase-6.md
git commit -m "docs: record codex-like runtime phase 6 verification"
```

## Phase 6 Acceptance Checklist

- [ ] Runtime has MCP-style external tool metadata.
- [ ] Runtime has connector metadata with auth-state placeholders.
- [ ] Built-in tools, runtime skills, and immediate external tools share one registry discovery model.
- [ ] Namespaced external tool names do not collide with built-ins.
- [ ] Registry rejects duplicate model-visible tool names.
- [ ] Deferred tools are advertised through discovery summaries.
- [ ] Deferred tools are not loaded into every model request as full schemas.
- [ ] Dev-only discovery endpoint reports tools and connectors.
- [ ] Documentation states that real MCP execution/auth/materialization remains later work.
- [ ] No edited/new source file exceeds 1000 physical lines.
