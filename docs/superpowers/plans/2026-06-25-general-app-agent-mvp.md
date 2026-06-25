# General App Agent MVP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> Repositioning note: Skill registry work in this plan is now an internal/developer-facing packaged capability path. Packaged apps hide skills from end users; desktop settings should expose model configuration only, with skill diagnostics limited to dev mode.

**Goal:** Build a headless-first general app agent MVP with configurable OpenAI-compatible model providers, developer-provided packaged skills, looping agent turns, and multi-session conversation storage.

**Architecture:** Use an Electron + React desktop client inspired by Hermes Desktop, backed by a Rust sidecar runtime inspired by Codex. Put protocol conversion and provider routing in a Rust Model Gateway that reuses cc-switch's provider adapter and Codex Responses/Chat bridge logic.

**Tech Stack:** Rust, Axum, Tokio, SQLx/SQLite, Electron, React, TypeScript, Vite, Vitest, Playwright, pixi.

---

## File Structure

Create this structure before implementation:

```text
.
├── pixi.toml
├── docs/
│   ├── feasibility.md
│   └── superpowers/plans/2026-06-25-general-app-agent-mvp.md
├── crates/
│   ├── agent-runtime/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── session.rs
│   │       ├── turn.rs
│   │       ├── events.rs
│   │       ├── storage.rs
│   │       └── skill.rs
│   ├── model-gateway/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── provider.rs
│   │       ├── adapter.rs
│   │       ├── chat.rs
│   │       ├── responses.rs
│   │       └── bridge.rs
│   └── agent-server/
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs
│           ├── api.rs
│           └── websocket.rs
├── apps/
│   └── desktop/
│       ├── package.json
│       ├── src/
│       │   ├── main/
│       │   ├── preload/
│       │   └── renderer/
│       └── tests/
└── skills/
    └── echo/
        ├── skill.json
        └── index.js
```

Responsibility boundaries:

- `crates/model-gateway`: Provider config, auth headers, protocol conversion, streaming normalization.
- `crates/agent-runtime`: Session model, conversation persistence, agent turn loop, skill registry and execution.
- `crates/agent-server`: Local HTTP/WebSocket API consumed by desktop or future clients.
- `apps/desktop`: Hermes-like chat, session, and model profile UI. Skill diagnostics, if any, are dev-only and hidden in packaged apps.
- `skills/echo`: Minimal developer command skill fixture used by tests and packaged registry demos.

## Task 1: Bootstrap Workspace

**Files:**

- Create: `pixi.toml`
- Create: `Cargo.toml`
- Create: `crates/model-gateway/Cargo.toml`
- Create: `crates/model-gateway/src/lib.rs`
- Create: `crates/agent-runtime/Cargo.toml`
- Create: `crates/agent-runtime/src/lib.rs`
- Create: `crates/agent-server/Cargo.toml`
- Create: `crates/agent-server/src/main.rs`
- Modify: `.gitignore`

- [ ] **Step 1: Create pixi environment**

Create `pixi.toml`:

```toml
[workspace]
channels = ["conda-forge"]
platforms = ["osx-arm64", "osx-64", "linux-64"]

[tasks]
fmt = "cargo fmt --all"
test = "cargo test --workspace"
server = "cargo run -p agent-server"

[dependencies]
nodejs = ">=22,<23"
rust = ">=1.86,<1.90"
pkg-config = ">=0.29"
openssl = ">=3"
sqlite = ">=3.45"
```

- [ ] **Step 2: Create Rust workspace**

Create root `Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = [
  "crates/model-gateway",
  "crates/agent-runtime",
  "crates/agent-server",
]

[workspace.package]
edition = "2024"
license = "Apache-2.0 OR MIT"

[workspace.dependencies]
anyhow = "1"
async-trait = "0.1"
axum = { version = "0.8", features = ["ws"] }
bytes = "1"
chrono = { version = "0.4", features = ["serde"] }
futures = "0.3"
reqwest = { version = "0.12", features = ["json", "stream"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite", "chrono", "json"] }
thiserror = "2"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "process", "time"] }
tokio-stream = "0.1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
uuid = { version = "1", features = ["v4", "serde"] }
```

- [ ] **Step 3: Create model-gateway crate**

Create `crates/model-gateway/Cargo.toml`:

```toml
[package]
name = "model-gateway"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
anyhow.workspace = true
async-trait.workspace = true
bytes.workspace = true
futures.workspace = true
reqwest.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tokio.workspace = true
tokio-stream.workspace = true
tracing.workspace = true
uuid.workspace = true
```

Create `crates/model-gateway/src/lib.rs`:

```rust
pub mod adapter;
pub mod bridge;
pub mod chat;
pub mod provider;
pub mod responses;
```

- [ ] **Step 4: Create agent-runtime crate**

Create `crates/agent-runtime/Cargo.toml`:

```toml
[package]
name = "agent-runtime"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
anyhow.workspace = true
async-trait.workspace = true
chrono.workspace = true
futures.workspace = true
model-gateway = { path = "../model-gateway" }
serde.workspace = true
serde_json.workspace = true
sqlx.workspace = true
thiserror.workspace = true
tokio.workspace = true
tokio-stream.workspace = true
tracing.workspace = true
uuid.workspace = true
```

Create `crates/agent-runtime/src/lib.rs`:

```rust
pub mod events;
pub mod session;
pub mod skill;
pub mod storage;
pub mod turn;
```

- [ ] **Step 5: Create agent-server crate**

Create `crates/agent-server/Cargo.toml`:

```toml
[package]
name = "agent-server"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
agent-runtime = { path = "../agent-runtime" }
anyhow.workspace = true
axum.workspace = true
serde.workspace = true
serde_json.workspace = true
tokio.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
uuid.workspace = true
```

Create `crates/agent-server/src/main.rs`:

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    println!("agent-server bootstrap");
    Ok(())
}
```

- [ ] **Step 6: Update .gitignore**

Ensure `.gitignore` contains:

```gitignore
.tool/
target/
.pixi/
node_modules/
dist/
*.db
*.db-shm
*.db-wal
```

- [ ] **Step 7: Verify bootstrap**

Run:

```bash
pixi run fmt
pixi run test
```

Expected:

```text
Finished test profile
```

- [ ] **Step 8: Commit**

```bash
git add .gitignore pixi.toml Cargo.toml crates
git commit -m "chore: bootstrap general agent workspace"
```

## Task 2: Define Model Gateway Provider Types

**Files:**

- Create: `crates/model-gateway/src/provider.rs`
- Create: `crates/model-gateway/src/adapter.rs`
- Modify: `crates/model-gateway/src/lib.rs`
- Test: `crates/model-gateway/src/provider.rs`

- [ ] **Step 1: Write provider model and tests**

Create `crates/model-gateway/src/provider.rs`:

```rust
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EndpointType {
    Responses,
    ChatCompletions,
    Completion,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ProviderProfile {
    pub id: String,
    pub name: String,
    pub endpoint_type: EndpointType,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

impl ProviderProfile {
    pub fn endpoint_path(&self) -> &'static str {
        match self.endpoint_type {
            EndpointType::Responses => "/responses",
            EndpointType::ChatCompletions => "/chat/completions",
            EndpointType::Completion => "/completions",
        }
    }

    pub fn endpoint_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        let path = self.endpoint_path().trim_start_matches('/');
        format!("{base}/{path}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_url_joins_base_and_path() {
        let profile = ProviderProfile {
            id: "local".into(),
            name: "Local".into(),
            endpoint_type: EndpointType::ChatCompletions,
            base_url: "http://localhost:11434/v1/".into(),
            model: "qwen".into(),
            api_key: None,
            headers: BTreeMap::new(),
        };

        assert_eq!(
            profile.endpoint_url(),
            "http://localhost:11434/v1/chat/completions"
        );
    }
}
```

- [ ] **Step 2: Write adapter trait**

Create `crates/model-gateway/src/adapter.rs`:

```rust
use crate::provider::ProviderProfile;
use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use serde_json::Value;
use std::pin::Pin;

pub type GatewayStream = Pin<Box<dyn Stream<Item = anyhow::Result<Bytes>> + Send>>;

#[async_trait]
pub trait ProviderAdapter: Send + Sync {
    fn name(&self) -> &'static str;
    fn supports(&self, profile: &ProviderProfile) -> bool;
    async fn stream(&self, profile: &ProviderProfile, request: Value) -> anyhow::Result<GatewayStream>;
}
```

- [ ] **Step 3: Run focused tests**

Run:

```bash
cargo test -p model-gateway provider::tests::endpoint_url_joins_base_and_path
```

Expected:

```text
test provider::tests::endpoint_url_joins_base_and_path ... ok
```

- [ ] **Step 4: Commit**

```bash
git add crates/model-gateway/src
git commit -m "feat: define model provider profiles"
```

## Task 3: Add Normalized Agent Events

**Files:**

- Create: `crates/model-gateway/src/responses.rs`
- Create: `crates/agent-runtime/src/events.rs`
- Test: `crates/agent-runtime/src/events.rs`

- [ ] **Step 1: Define gateway response items**

Create `crates/model-gateway/src/responses.rs`:

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GatewayEvent {
    ResponseStarted { response_id: String },
    TextDelta { text: String },
    ReasoningDelta { text: String },
    ToolCall {
        call_id: String,
        name: String,
        arguments: Value,
    },
    Usage { input_tokens: u64, output_tokens: u64 },
    Completed,
    Error { message: String },
}
```

- [ ] **Step 2: Define runtime events**

Create `crates/agent-runtime/src/events.rs`:

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeEvent {
    TurnStarted { turn_id: String },
    AssistantTextDelta { text: String },
    ReasoningDelta { text: String },
    ToolCallStarted {
        call_id: String,
        name: String,
        arguments: Value,
    },
    ToolCallFinished {
        call_id: String,
        result: Value,
    },
    AssistantMessageFinished { text: String },
    TurnFinished { turn_id: String },
    TurnFailed { turn_id: String, message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_event_serializes_with_snake_case_type() {
        let event = RuntimeEvent::AssistantTextDelta {
            text: "hello".into(),
        };

        let json = serde_json::to_value(event).unwrap();
        assert_eq!(json["type"], "assistant_text_delta");
        assert_eq!(json["text"], "hello");
    }
}
```

- [ ] **Step 3: Run event tests**

Run:

```bash
cargo test -p agent-runtime events::tests::runtime_event_serializes_with_snake_case_type
```

Expected:

```text
test events::tests::runtime_event_serializes_with_snake_case_type ... ok
```

- [ ] **Step 4: Commit**

```bash
git add crates/model-gateway/src/responses.rs crates/agent-runtime/src/events.rs
git commit -m "feat: add normalized runtime events"
```

## Task 4: Implement SQLite Session Storage

**Files:**

- Create: `crates/agent-runtime/src/session.rs`
- Create: `crates/agent-runtime/src/storage.rs`
- Test: `crates/agent-runtime/src/storage.rs`

- [ ] **Step 1: Define session entities**

Create `crates/agent-runtime/src/session.rs`:

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Message {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
}
```

- [ ] **Step 2: Implement storage**

Create `crates/agent-runtime/src/storage.rs`:

```rust
use crate::session::{Message, Session};
use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

#[derive(Clone)]
pub struct Storage {
    pool: SqlitePool,
}

impl Storage {
    pub async fn connect(url: &str) -> anyhow::Result<Self> {
        let pool = SqlitePool::connect(url).await?;
        let storage = Self { pool };
        storage.migrate().await?;
        Ok(storage)
    }

    async fn migrate(&self) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
              id TEXT PRIMARY KEY,
              title TEXT NOT NULL,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS messages (
              id TEXT PRIMARY KEY,
              session_id TEXT NOT NULL,
              role TEXT NOT NULL,
              content TEXT NOT NULL,
              created_at TEXT NOT NULL,
              FOREIGN KEY(session_id) REFERENCES sessions(id)
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn create_session(&self, title: &str) -> anyhow::Result<Session> {
        let now = Utc::now();
        let session = Session {
            id: Uuid::new_v4().to_string(),
            title: title.to_string(),
            created_at: now,
            updated_at: now,
        };

        sqlx::query("INSERT INTO sessions (id, title, created_at, updated_at) VALUES (?, ?, ?, ?)")
            .bind(&session.id)
            .bind(&session.title)
            .bind(session.created_at.to_rfc3339())
            .bind(session.updated_at.to_rfc3339())
            .execute(&self.pool)
            .await?;

        Ok(session)
    }

    pub async fn append_message(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
    ) -> anyhow::Result<Message> {
        let message = Message {
            id: Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            role: role.to_string(),
            content: content.to_string(),
            created_at: Utc::now(),
        };

        sqlx::query("INSERT INTO messages (id, session_id, role, content, created_at) VALUES (?, ?, ?, ?, ?)")
            .bind(&message.id)
            .bind(&message.session_id)
            .bind(&message.role)
            .bind(&message.content)
            .bind(message.created_at.to_rfc3339())
            .execute(&self.pool)
            .await?;

        Ok(message)
    }

    pub async fn list_messages(&self, session_id: &str) -> anyhow::Result<Vec<Message>> {
        let rows = sqlx::query(
            "SELECT id, session_id, role, content, created_at FROM messages WHERE session_id = ? ORDER BY created_at ASC",
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                let created_at: String = row.try_get("created_at")?;
                Ok(Message {
                    id: row.try_get("id")?,
                    session_id: row.try_get("session_id")?,
                    role: row.try_get("role")?,
                    content: row.try_get("content")?,
                    created_at: DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stores_and_lists_messages() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let session = storage.create_session("Test").await.unwrap();

        storage
            .append_message(&session.id, "user", "hello")
            .await
            .unwrap();

        let messages = storage.list_messages(&session.id).await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content, "hello");
    }
}
```

- [ ] **Step 3: Run storage tests**

Run:

```bash
cargo test -p agent-runtime storage::tests::stores_and_lists_messages
```

Expected:

```text
test storage::tests::stores_and_lists_messages ... ok
```

- [ ] **Step 4: Commit**

```bash
git add crates/agent-runtime/src/session.rs crates/agent-runtime/src/storage.rs
git commit -m "feat: store sessions and messages"
```

## Task 5: Implement Command Skill Registry

**Files:**

- Create: `crates/agent-runtime/src/skill.rs`
- Create: `skills/echo/skill.json`
- Create: `skills/echo/index.js`
- Test: `crates/agent-runtime/src/skill.rs`

- [ ] **Step 1: Create echo skill fixture**

Create `skills/echo/skill.json`:

```json
{
  "name": "echo",
  "description": "Echo a text payload.",
  "version": "0.1.0",
  "entry": {
    "type": "command",
    "command": "node",
    "args": ["index.js"]
  },
  "tools": [
    {
      "name": "echo",
      "description": "Return the provided text.",
      "input_schema": {
        "type": "object",
        "properties": {
          "text": { "type": "string" }
        },
        "required": ["text"]
      }
    }
  ]
}
```

Create `skills/echo/index.js`:

```javascript
const chunks = [];
process.stdin.on("data", (chunk) => chunks.push(chunk));
process.stdin.on("end", () => {
  const input = JSON.parse(Buffer.concat(chunks).toString("utf8"));
  process.stdout.write(JSON.stringify({ text: input.text }));
});
```

- [ ] **Step 2: Implement skill registry**

Create `crates/agent-runtime/src/skill.rs`:

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SkillManifest {
    pub name: String,
    pub description: String,
    pub version: String,
    pub entry: SkillEntry,
    pub tools: Vec<SkillTool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SkillEntry {
    #[serde(rename = "type")]
    pub kind: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SkillTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone)]
pub struct InstalledSkill {
    pub root: PathBuf,
    pub manifest: SkillManifest,
}

pub struct SkillRegistry {
    skills: Vec<InstalledSkill>,
}

impl SkillRegistry {
    pub async fn load(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        let mut skills = Vec::new();
        let mut entries = tokio::fs::read_dir(root).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let manifest_path = path.join("skill.json");
            if !manifest_path.exists() {
                continue;
            }

            let bytes = tokio::fs::read(&manifest_path).await?;
            let manifest: SkillManifest = serde_json::from_slice(&bytes)?;
            skills.push(InstalledSkill { root: path, manifest });
        }

        Ok(Self { skills })
    }

    pub fn tools(&self) -> Vec<SkillTool> {
        self.skills
            .iter()
            .flat_map(|skill| skill.manifest.tools.clone())
            .collect()
    }

    pub async fn execute(&self, tool_name: &str, input: Value) -> anyhow::Result<Value> {
        let skill = self
            .skills
            .iter()
            .find(|skill| skill.manifest.tools.iter().any(|tool| tool.name == tool_name))
            .ok_or_else(|| anyhow::anyhow!("unknown tool: {tool_name}"))?;

        let mut child = Command::new(&skill.manifest.entry.command)
            .args(&skill.manifest.entry.args)
            .current_dir(&skill.root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()?;

        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(serde_json::to_vec(&input)?.as_slice()).await?;
        drop(stdin);

        let output = child.wait_with_output().await?;
        if !output.status.success() {
            anyhow::bail!("skill command failed: {}", output.status);
        }

        Ok(serde_json::from_slice(&output.stdout)?)
    }
}
```

- [ ] **Step 3: Add registry test**

Append this test module to `crates/agent-runtime/src/skill.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn loads_and_executes_echo_skill() {
        let registry = SkillRegistry::load("../../skills").await.unwrap();
        let tools = registry.tools();
        assert!(tools.iter().any(|tool| tool.name == "echo"));

        let result = registry
            .execute("echo", serde_json::json!({ "text": "hello" }))
            .await
            .unwrap();

        assert_eq!(result["text"], "hello");
    }
}
```

- [ ] **Step 4: Run skill tests**

Run:

```bash
cargo test -p agent-runtime skill::tests::loads_and_executes_echo_skill
```

Expected:

```text
test skill::tests::loads_and_executes_echo_skill ... ok
```

- [ ] **Step 5: Commit**

```bash
git add crates/agent-runtime/src/skill.rs skills/echo
git commit -m "feat: add command skill registry"
```

## Task 6: Implement Minimal Agent Turn Loop

**Files:**

- Create: `crates/agent-runtime/src/turn.rs`
- Modify: `crates/agent-runtime/src/lib.rs`
- Test: `crates/agent-runtime/src/turn.rs`

- [ ] **Step 1: Define model client abstraction and turn loop**

Create `crates/agent-runtime/src/turn.rs`:

```rust
use crate::events::RuntimeEvent;
use crate::skill::SkillRegistry;
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use model_gateway::responses::GatewayEvent;
use serde_json::Value;
use std::pin::Pin;
use uuid::Uuid;

pub type ModelEventStream = Pin<Box<dyn Stream<Item = anyhow::Result<GatewayEvent>> + Send>>;

#[async_trait]
pub trait ModelClient: Send + Sync {
    async fn stream(&self, input: Vec<Value>) -> anyhow::Result<ModelEventStream>;
}

pub struct TurnRunner<C> {
    model: C,
    skills: SkillRegistry,
    max_steps: usize,
}

impl<C> TurnRunner<C>
where
    C: ModelClient,
{
    pub fn new(model: C, skills: SkillRegistry) -> Self {
        Self {
            model,
            skills,
            max_steps: 8,
        }
    }

    pub async fn run(&self, user_text: &str) -> anyhow::Result<Vec<RuntimeEvent>> {
        let turn_id = Uuid::new_v4().to_string();
        let mut events = vec![RuntimeEvent::TurnStarted {
            turn_id: turn_id.clone(),
        }];
        let mut input = vec![serde_json::json!({ "role": "user", "content": user_text })];
        let mut final_text = String::new();

        for _step in 0..self.max_steps {
            let mut stream = self.model.stream(input.clone()).await?;
            let mut saw_tool = false;

            while let Some(event) = stream.next().await {
                match event? {
                    GatewayEvent::TextDelta { text } => {
                        final_text.push_str(&text);
                        events.push(RuntimeEvent::AssistantTextDelta { text });
                    }
                    GatewayEvent::ReasoningDelta { text } => {
                        events.push(RuntimeEvent::ReasoningDelta { text });
                    }
                    GatewayEvent::ToolCall {
                        call_id,
                        name,
                        arguments,
                    } => {
                        saw_tool = true;
                        events.push(RuntimeEvent::ToolCallStarted {
                            call_id: call_id.clone(),
                            name: name.clone(),
                            arguments: arguments.clone(),
                        });
                        let result = self.skills.execute(&name, arguments).await?;
                        events.push(RuntimeEvent::ToolCallFinished {
                            call_id: call_id.clone(),
                            result: result.clone(),
                        });
                        input.push(serde_json::json!({
                            "role": "tool",
                            "tool_call_id": call_id,
                            "content": result
                        }));
                    }
                    GatewayEvent::Completed => {}
                    GatewayEvent::Error { message } => {
                        events.push(RuntimeEvent::TurnFailed {
                            turn_id: turn_id.clone(),
                            message,
                        });
                        return Ok(events);
                    }
                    GatewayEvent::ResponseStarted { .. } | GatewayEvent::Usage { .. } => {}
                }
            }

            if !saw_tool {
                events.push(RuntimeEvent::AssistantMessageFinished { text: final_text });
                events.push(RuntimeEvent::TurnFinished { turn_id });
                return Ok(events);
            }
        }

        events.push(RuntimeEvent::TurnFailed {
            turn_id,
            message: "max agent steps exceeded".into(),
        });
        Ok(events)
    }
}
```

- [ ] **Step 2: Add turn loop test**

Append this test module to `crates/agent-runtime/src/turn.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeModel {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl ModelClient for FakeModel {
        async fn stream(&self, _input: Vec<Value>) -> anyhow::Result<ModelEventStream> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            let events = if call == 0 {
                vec![
                    Ok(GatewayEvent::ToolCall {
                        call_id: "call-1".into(),
                        name: "echo".into(),
                        arguments: serde_json::json!({ "text": "hello" }),
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

    #[tokio::test]
    async fn executes_tool_and_continues_until_text_response() {
        let skills = SkillRegistry::load("../../skills").await.unwrap();
        let runner = TurnRunner::new(
            FakeModel {
                calls: AtomicUsize::new(0),
            },
            skills,
        );

        let events = runner.run("echo hello").await.unwrap();

        assert!(matches!(
            events.last(),
            Some(RuntimeEvent::TurnFinished { .. })
        ));
        assert!(events.iter().any(|event| matches!(
            event,
            RuntimeEvent::ToolCallFinished { call_id, .. } if call_id == "call-1"
        )));
    }
}
```

- [ ] **Step 3: Run turn loop tests**

Run:

```bash
cargo test -p agent-runtime turn::tests::executes_tool_and_continues_until_text_response
```

Expected:

```text
test turn::tests::executes_tool_and_continues_until_text_response ... ok
```

- [ ] **Step 4: Commit**

```bash
git add crates/agent-runtime/src/turn.rs
git commit -m "feat: implement minimal agent turn loop"
```

## Task 7: Add Headless HTTP API

**Files:**

- Create: `crates/agent-server/src/api.rs`
- Modify: `crates/agent-server/src/main.rs`
- Test: `crates/agent-server/src/api.rs`

- [ ] **Step 1: Implement API routes**

Create `crates/agent-server/src/api.rs`:

```rust
use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct AppState;

#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub title: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreateSessionResponse {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Deserialize)]
pub struct UserMessageRequest {
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct UserMessageResponse {
    pub accepted: bool,
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/sessions", post(create_session))
        .route("/sessions/{session_id}/messages", post(post_message))
        .with_state(state)
}

async fn create_session(
    State(_state): State<Arc<AppState>>,
    Json(request): Json<CreateSessionRequest>,
) -> Json<CreateSessionResponse> {
    Json(CreateSessionResponse {
        id: uuid::Uuid::new_v4().to_string(),
        title: request.title.unwrap_or_else(|| "New Session".to_string()),
    })
}

async fn post_message(
    Path(_session_id): Path<String>,
    State(_state): State<Arc<AppState>>,
    Json(_request): Json<UserMessageRequest>,
) -> Json<UserMessageResponse> {
    Json(UserMessageResponse { accepted: true })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_returns_ok() {
        let app = router(Arc::new(AppState));
        let response = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
```

- [ ] **Step 2: Wire server main**

Replace `crates/agent-server/src/main.rs` with:

```rust
mod api;

use std::{net::SocketAddr, sync::Arc};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let app = api::router(Arc::new(api::AppState));
    let addr = SocketAddr::from(([127, 0, 0, 1], 49321));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    tracing::info!("agent server listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
```

- [ ] **Step 3: Add tower dependency**

Modify `crates/agent-server/Cargo.toml`:

```toml
tower = "0.5"
```

- [ ] **Step 4: Run API tests**

Run:

```bash
cargo test -p agent-server api::tests::health_returns_ok
```

Expected:

```text
test api::tests::health_returns_ok ... ok
```

- [ ] **Step 5: Commit**

```bash
git add crates/agent-server
git commit -m "feat: expose headless agent api"
```

## Task 8: Port cc-switch Bridge Incrementally

**Files:**

- Modify: `crates/model-gateway/src/bridge.rs`
- Source reference: `.tool/cc-switch/src-tauri/src/proxy/providers/transform_codex_chat.rs`
- Source reference: `.tool/cc-switch/src-tauri/src/proxy/providers/streaming_codex_chat.rs`
- Source reference: `.tool/cc-switch/src-tauri/src/proxy/providers/codex_chat_common.rs`
- Test: `crates/model-gateway/src/bridge.rs`

- [ ] **Step 1: Copy only the minimum conversion helpers**

Start by porting the smallest path:

- text-only Responses input to Chat Completions messages.
- Chat text delta to `GatewayEvent::TextDelta`.
- Chat finish reason to `GatewayEvent::Completed`.

Do not port every provider-specific branch in the first commit.

- [ ] **Step 2: Add text-only conversion test**

Add this test to `crates/model-gateway/src/bridge.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_responses_text_input_to_chat_messages() {
        let input = serde_json::json!({
            "model": "test-model",
            "input": [
                { "role": "user", "content": "hello" }
            ]
        });

        let chat = responses_to_chat_completions(input).unwrap();

        assert_eq!(chat["model"], "test-model");
        assert_eq!(chat["messages"][0]["role"], "user");
        assert_eq!(chat["messages"][0]["content"], "hello");
    }
}
```

- [ ] **Step 3: Run bridge tests**

Run:

```bash
cargo test -p model-gateway bridge::tests::converts_responses_text_input_to_chat_messages
```

Expected:

```text
test bridge::tests::converts_responses_text_input_to_chat_messages ... ok
```

- [ ] **Step 4: Expand bridge in small commits**

Port these cc-switch capabilities one by one, each with a focused test:

- function tools
- namespace tools
- custom tools
- tool call arguments
- reasoning deltas
- usage conversion
- streaming tool call assembly

- [ ] **Step 5: Commit**

```bash
git add crates/model-gateway/src/bridge.rs
git commit -m "feat: bridge responses and chat completions"
```

## Task 9: Desktop Client Skeleton

**Files:**

- Create: `apps/desktop/package.json`
- Create: `apps/desktop/src/renderer/App.tsx`
- Create: `apps/desktop/src/renderer/screens/Chat.tsx`
- Create: `apps/desktop/src/renderer/screens/Sessions.tsx`
- Create: `apps/desktop/src/preload/index.ts`
- Create: `apps/desktop/src/main/index.ts`
- Test: `apps/desktop/tests/chat.test.tsx`

- [ ] **Step 1: Create package**

Create `apps/desktop/package.json`:

```json
{
  "name": "general-agent-desktop",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "scripts": {
    "dev": "vite",
    "test": "vitest run"
  },
  "dependencies": {
    "@vitejs/plugin-react": "^5.0.0",
    "vite": "^7.0.0",
    "react": "^19.0.0",
    "react-dom": "^19.0.0",
    "lucide-react": "^0.468.0"
  },
  "devDependencies": {
    "@testing-library/react": "^16.0.0",
    "@testing-library/jest-dom": "^6.0.0",
    "typescript": "^5.7.0",
    "vitest": "^3.0.0"
  }
}
```

- [ ] **Step 2: Create minimal chat screen**

Create `apps/desktop/src/renderer/screens/Chat.tsx`:

```tsx
import { Send } from "lucide-react";
import { useState } from "react";

type Message = {
  id: string;
  role: "user" | "assistant";
  content: string;
};

export function Chat() {
  const [messages, setMessages] = useState<Message[]>([]);
  const [draft, setDraft] = useState("");

  function send() {
    const content = draft.trim();
    if (!content) return;
    setMessages((current) => [
      ...current,
      { id: crypto.randomUUID(), role: "user", content },
    ]);
    setDraft("");
  }

  return (
    <main>
      <section aria-label="Conversation">
        {messages.map((message) => (
          <article key={message.id} data-role={message.role}>
            {message.content}
          </article>
        ))}
      </section>
      <footer>
        <textarea
          aria-label="Message"
          value={draft}
          onChange={(event) => setDraft(event.currentTarget.value)}
        />
        <button aria-label="Send message" onClick={send}>
          <Send size={18} />
        </button>
      </footer>
    </main>
  );
}
```

- [ ] **Step 3: Create App entry**

Create `apps/desktop/src/renderer/App.tsx`:

```tsx
import { Chat } from "./screens/Chat";

export function App() {
  return <Chat />;
}
```

- [ ] **Step 4: Add chat test**

Create `apps/desktop/tests/chat.test.tsx`:

```tsx
import "@testing-library/jest-dom/vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Chat } from "../src/renderer/screens/Chat";

describe("Chat", () => {
  it("adds a user message", () => {
    render(<Chat />);
    fireEvent.change(screen.getByLabelText("Message"), {
      target: { value: "hello" },
    });
    fireEvent.click(screen.getByLabelText("Send message"));
    expect(screen.getByText("hello")).toBeInTheDocument();
  });
});
```

- [ ] **Step 5: Run desktop test**

Run:

```bash
cd apps/desktop && npm test -- --run tests/chat.test.tsx
```

Expected:

```text
1 passed
```

- [ ] **Step 6: Commit**

```bash
git add apps/desktop
git commit -m "feat: add desktop chat skeleton"
```

## Task 10: End-to-End MVP Verification

**Files:**

- Create: `docs/mvp-verification.md`
- Modify: `crates/agent-server/src/api.rs`
- Modify: `apps/desktop/src/renderer/screens/Chat.tsx`

- [ ] **Step 1: Connect desktop to server**

Update Chat screen to call:

```text
POST http://127.0.0.1:49321/sessions
POST http://127.0.0.1:49321/sessions/:id/messages
```

Display assistant deltas from WebSocket or polling endpoint once Task 7 grows streaming support.

- [ ] **Step 2: Run server**

Run:

```bash
pixi run server
```

Expected:

```text
agent server listening on http://127.0.0.1:49321
```

- [ ] **Step 3: Run full Rust tests**

Run:

```bash
pixi run test
```

Expected:

```text
test result: ok
```

- [ ] **Step 4: Run desktop tests**

Run:

```bash
cd apps/desktop && npm test
```

Expected:

```text
Test Files  1 passed
```

- [ ] **Step 5: Write verification note**

Create `docs/mvp-verification.md`:

```markdown
# MVP Verification

Date: 2026-06-25

## Automated Checks

- `pixi run test`: passed
- `cd apps/desktop && npm test`: passed

## Manual Checks

- Created a session.
- Sent a user message.
- Observed agent turn events.
- Observed skill call and result.
- Restored the session from history.

## Known Gaps

- Completion endpoint is text-only in MVP.
- Skill sandboxing is command timeout only.
- Provider failover starts with static routing.
```

- [ ] **Step 6: Commit**

```bash
git add docs/mvp-verification.md crates apps
git commit -m "test: verify mvp chat loop"
```

## Self-Review

Spec coverage:

- Configurable OpenAI-compatible providers: covered by Tasks 2 and 8.
- Chat/Completion/Responses shape: covered by provider endpoint types and bridge task.
- Pluggable skills: covered by Task 5.
- Agent loop: covered by Task 6.
- Multi session/conversation: covered by Task 4 and Task 7.
- Reuse from Codex/Hermes/cc-switch: captured in `docs/feasibility.md` and explicitly represented in Tasks 6, 8, and 9.

Placeholder scan:

- No banned placeholder markers remain.
- The cc-switch bridge is intentionally incremental, with the exact source files and first test specified.

Type consistency:

- `GatewayEvent` is defined in Task 3 before `TurnRunner` uses it in Task 6.
- `SkillRegistry` is defined in Task 5 before `TurnRunner` uses it in Task 6.
- `ProviderProfile` and `EndpointType` are defined in Task 2 before gateway bridge work.

Execution handoff:

Plan complete and saved to `docs/superpowers/plans/2026-06-25-general-app-agent-mvp.md`.

Two execution options:

1. Subagent-Driven (recommended): dispatch a fresh subagent per task, review between tasks, fast iteration.
2. Inline Execution: execute tasks in this session using `superpowers:executing-plans`, batch execution with checkpoints.
