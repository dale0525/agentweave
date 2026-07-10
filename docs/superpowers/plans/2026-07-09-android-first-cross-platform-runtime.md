# Android-First Cross-Platform Runtime Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build an Android-first GeneralAgent MVP that runs full agent turns locally through the shared Rust runtime, with Android providing only UI, app directories, capabilities, and Keystore secrets.

**Architecture:** The Rust core owns sessions, messages, model calls, skill filtering, tool dispatch, VFS, and runtime events. Android hosts the core through `crates/mobile-ffi`, registers platform capabilities, stores secrets in Keystore, and renders Compose screens from FFI DTOs. The MVP uses HTTP model providers and app-private file access only.

**Tech Stack:** Rust 2024, Tokio, SQLx/SQLite, serde, reqwest, UniFFI, Android Gradle Plugin 9.2.1, Kotlin 2.4.0, Jetpack Compose BOM 2026.06.01, Android Keystore, pixi tasks.

## Global Constraints

- Android must complete an agent turn without desktop or `agent-server`.
- MVP model path is HTTP provider access.
- API keys must be stored in Android Keystore and must not be written to Rust SQLite, files, or logs.
- Android MVP registers only `network.http`, `filesystem.app_data`, `secure_storage`, and `model.http_provider`.
- Android MVP does not support iOS, desktop-only tools, arbitrary file-system access, shell/process execution, headless browser automation, background long-running turns, push notifications, camera, screen, voice, canvas, node invocation, or OpenClaw Gateway compatibility.
- Runtime tools without explicit capability metadata are disabled on Android until they declare their requirements.
- Mobile file access must go through app-data VFS URIs and reject absolute paths, `..` traversal, and symlink escape.
- Use `pixi` for developer tasks and avoid system-wide tool installation.
- Source-like files must stay under 1000 physical lines.
- Use English for code and technical identifiers.

---

## Scope Check

This plan implements the Android MVP from the approved design. It deliberately excludes iOS and OpenClaw Gateway compatibility. The work is split so each task has one independently testable result:

1. platform capability primitives
2. skill availability filtering
3. app-data VFS
4. Rust-owned mobile runtime host
5. HTTP model config and secret bridge
6. mobile FFI facade
7. Android scaffold and native bridge
8. Keystore-backed model settings
9. Compose MVP screens
10. end-to-end verification tasks

## File Structure

Create or modify these units:

- `crates/agent-runtime/src/platform.rs`: platform IDs and capability-set primitives.
- `crates/agent-runtime/src/skill_availability.rs`: capability metadata parsing and availability decisions.
- `crates/agent-runtime/src/vfs.rs`: app-data VFS resolver and path safety checks.
- `crates/agent-runtime/src/mobile_host.rs`: Rust-owned runtime host used by mobile and future platform shells.
- `crates/agent-runtime/src/model_config.rs`: non-secret model configuration persistence types.
- `crates/agent-runtime/src/storage.rs`: add session listing/deletion helpers already needed by mobile.
- `crates/agent-runtime/src/lib.rs`: export new modules.
- `crates/mobile-ffi/Cargo.toml`: mobile facade crate.
- `crates/mobile-ffi/src/lib.rs`: FFI-safe runtime facade.
- `crates/mobile-ffi/src/types.rs`: DTOs exposed to Kotlin.
- `crates/mobile-ffi/src/runtime.rs`: internal wrapper around `agent_runtime::mobile_host`.
- `crates/mobile-ffi/tests/mobile_runtime.rs`: facade-level Rust tests.
- `Cargo.toml`: include `crates/mobile-ffi` and shared dependencies.
- `pixi.toml`: add Android and mobile FFI tasks.
- `apps/android/settings.gradle.kts`: Android project settings.
- `apps/android/build.gradle.kts`: Android root Gradle config.
- `apps/android/gradle/libs.versions.toml`: pinned Android dependency versions.
- `apps/android/app/build.gradle.kts`: Android app module config.
- `apps/android/app/src/main/AndroidManifest.xml`: app manifest.
- `apps/android/app/src/main/java/com/generalagent/mobile/MainActivity.kt`: Compose entry.
- `apps/android/app/src/main/java/com/generalagent/mobile/runtime/RuntimeBridge.kt`: Kotlin wrapper for FFI calls.
- `apps/android/app/src/main/java/com/generalagent/mobile/runtime/AndroidCapabilities.kt`: Android capability registration.
- `apps/android/app/src/main/java/com/generalagent/mobile/secrets/ModelSecretStore.kt`: Keystore storage.
- `apps/android/app/src/main/java/com/generalagent/mobile/ui/ChatScreen.kt`: Chat MVP.
- `apps/android/app/src/main/java/com/generalagent/mobile/ui/SettingsScreen.kt`: HTTP model config.
- `apps/android/app/src/main/java/com/generalagent/mobile/ui/SkillsScreen.kt`: skill availability.
- `apps/android/app/src/main/java/com/generalagent/mobile/ui/DiagnosticsScreen.kt`: runtime diagnostics.
- `apps/android/app/src/test/java/com/generalagent/mobile/...`: Robolectric/JUnit tests for bridge, Keystore wrapper, and UI state.

### Task 1: Platform Capability Primitives

**Files:**
- Create: `crates/agent-runtime/src/platform.rs`
- Modify: `crates/agent-runtime/src/lib.rs`
- Test: `crates/agent-runtime/src/platform.rs`

**Interfaces:**
- Produces: `PlatformId`, `Capability`, `CapabilitySet`, `CapabilitySet::android_mvp()`, `CapabilitySet::contains_name(&str) -> bool`
- Consumes: no earlier task

- [ ] **Step 1: Write failing tests**

Add this test module to the new file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn android_mvp_registers_only_mobile_safe_core_capabilities() {
        let capabilities = CapabilitySet::android_mvp();

        assert!(capabilities.contains_name("network.http"));
        assert!(capabilities.contains_name("filesystem.app_data"));
        assert!(capabilities.contains_name("secure_storage"));
        assert!(capabilities.contains_name("model.http_provider"));
        assert!(!capabilities.contains_name("shell.process"));
        assert!(!capabilities.contains_name("browser.headless"));
        assert!(!capabilities.contains_name("desktop.automation"));
        assert!(!capabilities.contains_name("filesystem.unrestricted"));
    }

    #[test]
    fn capability_names_are_trimmed_and_deduplicated() {
        let capabilities = CapabilitySet::from_names([
            " network.http ",
            "network.http",
            "",
            "filesystem.app_data",
        ]);

        assert_eq!(capabilities.names(), &["filesystem.app_data", "network.http"]);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `pixi run test -p agent-runtime platform`

Expected: FAIL because `platform` module is not exported or the new types do not exist.

- [ ] **Step 3: Implement capability primitives**

Create `crates/agent-runtime/src/platform.rs`:

```rust
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlatformId {
    Desktop,
    Android,
    Ios,
    Web,
    Server,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Capability(String);

impl Capability {
    pub fn new(name: impl Into<String>) -> Option<Self> {
        let name = name.into().trim().to_string();
        if name.is_empty() {
            return None;
        }
        Some(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct CapabilitySet {
    names: Vec<String>,
}

impl CapabilitySet {
    pub fn from_names<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut unique = BTreeSet::new();
        for name in names {
            if let Some(capability) = Capability::new(name) {
                unique.insert(capability.as_str().to_string());
            }
        }
        Self {
            names: unique.into_iter().collect(),
        }
    }

    pub fn android_mvp() -> Self {
        Self::from_names([
            "network.http",
            "filesystem.app_data",
            "secure_storage",
            "model.http_provider",
        ])
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }

    pub fn contains_name(&self, name: &str) -> bool {
        self.names.iter().any(|item| item == name)
    }
}
```

Modify `crates/agent-runtime/src/lib.rs`:

```rust
pub mod context;
pub mod events;
pub mod instructions;
pub mod platform;
pub mod policy;
pub mod session;
pub mod skill;
pub mod skill_catalog;
pub mod storage;
pub mod subagent;
pub mod tools;
pub mod turn;
pub mod turn_request;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `pixi run test -p agent-runtime platform`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/agent-runtime/src/lib.rs crates/agent-runtime/src/platform.rs
git commit -m "feat: add platform capability primitives"
```

### Task 2: Skill Availability Filtering

**Files:**
- Create: `crates/agent-runtime/src/skill_availability.rs`
- Modify: `crates/agent-runtime/src/lib.rs`
- Modify: `crates/agent-runtime/src/skill.rs`
- Test: `crates/agent-runtime/src/skill_availability.rs`

**Interfaces:**
- Consumes: `CapabilitySet` from Task 1
- Produces: `SkillCapabilityMetadata`, `SkillAvailability`, `SkillAvailabilityStatus`, `evaluate_skill_availability(...)`

- [ ] **Step 1: Write failing tests**

Create `crates/agent-runtime/src/skill_availability.rs` with the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{CapabilitySet, PlatformId};

    #[test]
    fn disables_skill_when_required_capability_is_missing() {
        let metadata = SkillCapabilityMetadata {
            requires: vec!["network.http".into(), "browser.headless".into()],
            optional: vec![],
            platforms: PlatformOverrides::default(),
        };
        let availability = evaluate_skill_availability(
            "web-search",
            &metadata,
            PlatformId::Android,
            &CapabilitySet::android_mvp(),
            false,
        );

        assert_eq!(availability.status, SkillAvailabilityStatus::Unavailable);
        assert_eq!(availability.missing_capabilities, vec!["browser.headless"]);
        assert_eq!(
            availability.reason,
            "Missing required capability: browser.headless"
        );
    }

    #[test]
    fn platform_override_uses_human_reason() {
        let metadata = SkillCapabilityMetadata {
            requires: vec!["network.http".into()],
            optional: vec![],
            platforms: PlatformOverrides {
                android: Some(PlatformSkillOverride {
                    status: PlatformSkillStatus::Unsupported,
                    reason: "Requires a desktop headless browser.".into(),
                }),
            },
        };
        let availability = evaluate_skill_availability(
            "web-search",
            &metadata,
            PlatformId::Android,
            &CapabilitySet::android_mvp(),
            false,
        );

        assert_eq!(availability.status, SkillAvailabilityStatus::Unsupported);
        assert_eq!(availability.reason, "Requires a desktop headless browser.");
    }

    #[test]
    fn android_disables_runtime_tools_without_metadata() {
        let metadata = SkillCapabilityMetadata::default();
        let availability = evaluate_skill_availability(
            "legacy-tool-skill",
            &metadata,
            PlatformId::Android,
            &CapabilitySet::android_mvp(),
            true,
        );

        assert_eq!(availability.status, SkillAvailabilityStatus::Unavailable);
        assert_eq!(
            availability.reason,
            "Runtime tools must declare capability requirements on Android."
        );
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `pixi run test -p agent-runtime skill_availability`

Expected: FAIL because the new module is incomplete.

- [ ] **Step 3: Implement availability types and evaluator**

Add this implementation above the test module:

```rust
use crate::platform::{CapabilitySet, PlatformId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct SkillCapabilityMetadata {
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub optional: Vec<String>,
    #[serde(default)]
    pub platforms: PlatformOverrides,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct PlatformOverrides {
    #[serde(default)]
    pub android: Option<PlatformSkillOverride>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PlatformSkillOverride {
    pub status: PlatformSkillStatus,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlatformSkillStatus {
    Available,
    Unsupported,
    Unavailable,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillAvailabilityStatus {
    Available,
    Unavailable,
    Unsupported,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct SkillAvailability {
    pub skill_id: String,
    pub status: SkillAvailabilityStatus,
    pub missing_capabilities: Vec<String>,
    pub reason: String,
}

pub fn evaluate_skill_availability(
    skill_id: &str,
    metadata: &SkillCapabilityMetadata,
    platform: PlatformId,
    capabilities: &CapabilitySet,
    contributes_runtime_tools: bool,
) -> SkillAvailability {
    if let Some(override_status) = platform_override(metadata, platform) {
        return SkillAvailability {
            skill_id: skill_id.to_string(),
            status: match override_status.status {
                PlatformSkillStatus::Available => SkillAvailabilityStatus::Available,
                PlatformSkillStatus::Unavailable => SkillAvailabilityStatus::Unavailable,
                PlatformSkillStatus::Unsupported => SkillAvailabilityStatus::Unsupported,
            },
            missing_capabilities: Vec::new(),
            reason: override_status.reason.clone(),
        };
    }

    if platform == PlatformId::Android
        && contributes_runtime_tools
        && metadata.requires.is_empty()
        && metadata.optional.is_empty()
    {
        return SkillAvailability {
            skill_id: skill_id.to_string(),
            status: SkillAvailabilityStatus::Unavailable,
            missing_capabilities: Vec::new(),
            reason: "Runtime tools must declare capability requirements on Android.".into(),
        };
    }

    let missing: Vec<String> = metadata
        .requires
        .iter()
        .filter(|name| !capabilities.contains_name(name))
        .cloned()
        .collect();

    if missing.is_empty() {
        return SkillAvailability {
            skill_id: skill_id.to_string(),
            status: SkillAvailabilityStatus::Available,
            missing_capabilities: Vec::new(),
            reason: "Available on this platform.".into(),
        };
    }

    SkillAvailability {
        skill_id: skill_id.to_string(),
        status: SkillAvailabilityStatus::Unavailable,
        reason: if missing.len() == 1 {
            format!("Missing required capability: {}", missing[0])
        } else {
            format!("Missing required capabilities: {}", missing.join(", "))
        },
        missing_capabilities: missing,
    }
}

fn platform_override<'a>(
    metadata: &'a SkillCapabilityMetadata,
    platform: PlatformId,
) -> Option<&'a PlatformSkillOverride> {
    match platform {
        PlatformId::Android => metadata.platforms.android.as_ref(),
        PlatformId::Desktop | PlatformId::Ios | PlatformId::Web | PlatformId::Server => None,
    }
}
```

Modify `crates/agent-runtime/src/lib.rs`:

```rust
pub mod skill_availability;
```

Add this field to the runtime skill package metadata type in `crates/agent-runtime/src/skill.rs` after locating the package metadata struct:

```rust
#[serde(default)]
pub capabilities: crate::skill_availability::SkillCapabilityMetadata,
```

If the existing metadata struct is private, add a public accessor:

```rust
pub fn capability_metadata(&self) -> &crate::skill_availability::SkillCapabilityMetadata {
    &self.capabilities
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `pixi run test -p agent-runtime skill_availability`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/agent-runtime/src/lib.rs crates/agent-runtime/src/skill.rs crates/agent-runtime/src/skill_availability.rs
git commit -m "feat: filter skills by platform capabilities"
```

### Task 3: App-Data VFS

**Files:**
- Create: `crates/agent-runtime/src/vfs.rs`
- Modify: `crates/agent-runtime/src/lib.rs`
- Test: `crates/agent-runtime/src/vfs.rs`

**Interfaces:**
- Consumes: no earlier runtime host APIs
- Produces: `AppDataVfs`, `VfsRoot`, `VfsError`, `AppDataVfs::resolve_uri(&str) -> Result<PathBuf, VfsError>`

- [ ] **Step 1: Write failing tests**

Create `crates/agent-runtime/src/vfs.rs` with this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn resolves_documents_uri_inside_app_root() {
        let vfs = AppDataVfs::new("/app/files/documents", "/app/files/cache");
        assert_eq!(
            vfs.resolve_uri("app://documents/notes/today.md").unwrap(),
            PathBuf::from("/app/files/documents/notes/today.md")
        );
    }

    #[test]
    fn rejects_absolute_paths() {
        let vfs = AppDataVfs::new("/app/files/documents", "/app/files/cache");
        assert_eq!(
            vfs.resolve_uri("/etc/passwd").unwrap_err(),
            VfsError::UnsupportedScheme
        );
    }

    #[test]
    fn rejects_traversal() {
        let vfs = AppDataVfs::new("/app/files/documents", "/app/files/cache");
        assert_eq!(
            vfs.resolve_uri("app://documents/../secrets.txt").unwrap_err(),
            VfsError::PathTraversal
        );
    }

    #[test]
    fn rejects_unknown_app_root() {
        let vfs = AppDataVfs::new("/app/files/documents", "/app/files/cache");
        assert_eq!(
            vfs.resolve_uri("app://skills/SKILL.md").unwrap_err(),
            VfsError::UnsupportedRoot
        );
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `pixi run test -p agent-runtime vfs`

Expected: FAIL because `AppDataVfs` does not exist.

- [ ] **Step 3: Implement app-data VFS**

Add this implementation above the test module:

```rust
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AppDataVfs {
    documents_root: PathBuf,
    cache_root: PathBuf,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum VfsRoot {
    Documents,
    Cache,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum VfsError {
    UnsupportedScheme,
    UnsupportedRoot,
    EmptyPath,
    PathTraversal,
}

impl AppDataVfs {
    pub fn new(documents_root: impl Into<PathBuf>, cache_root: impl Into<PathBuf>) -> Self {
        Self {
            documents_root: documents_root.into(),
            cache_root: cache_root.into(),
        }
    }

    pub fn resolve_uri(&self, uri: &str) -> Result<PathBuf, VfsError> {
        let rest = uri
            .strip_prefix("app://")
            .ok_or(VfsError::UnsupportedScheme)?;
        let (root_name, relative) = rest.split_once('/').ok_or(VfsError::EmptyPath)?;
        let root = match root_name {
            "documents" => VfsRoot::Documents,
            "cache" => VfsRoot::Cache,
            _ => return Err(VfsError::UnsupportedRoot),
        };
        let safe_relative = safe_relative_path(relative)?;
        Ok(match root {
            VfsRoot::Documents => self.documents_root.join(safe_relative),
            VfsRoot::Cache => self.cache_root.join(safe_relative),
        })
    }
}

fn safe_relative_path(value: &str) -> Result<PathBuf, VfsError> {
    if value.trim().is_empty() {
        return Err(VfsError::EmptyPath);
    }
    let path = Path::new(value);
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(segment) => result.push(segment),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(VfsError::PathTraversal);
            }
        }
    }
    if result.as_os_str().is_empty() {
        return Err(VfsError::EmptyPath);
    }
    Ok(result)
}
```

Modify `crates/agent-runtime/src/lib.rs`:

```rust
pub mod vfs;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `pixi run test -p agent-runtime vfs`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/agent-runtime/src/lib.rs crates/agent-runtime/src/vfs.rs
git commit -m "feat: add app-data vfs"
```

### Task 4: Rust-Owned Mobile Runtime Host

**Files:**
- Create: `crates/agent-runtime/src/model_config.rs`
- Create: `crates/agent-runtime/src/mobile_host.rs`
- Modify: `crates/agent-runtime/src/lib.rs`
- Modify: `crates/agent-runtime/src/storage.rs`
- Test: `crates/agent-runtime/src/mobile_host.rs`
- Test: `crates/agent-runtime/src/storage.rs`

**Interfaces:**
- Consumes: `CapabilitySet`, `AppDataVfs`, `SkillAvailability`
- Produces: `MobileRuntimeHost`, `MobileRuntimeInit`, `MobileRuntimeDiagnostics`, `SecretResolver`, `MobileRuntimeHost::send_message(...)`

- [ ] **Step 1: Add storage tests for mobile session needs**

Append these tests in `crates/agent-runtime/src/storage.rs`:

```rust
#[tokio::test]
async fn list_sessions_returns_newest_updated_first() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let first = storage.create_session("First").await.unwrap();
    let second = storage.create_session("Second").await.unwrap();
    storage.append_message(&first.id, "user", "hello").await.unwrap();

    let sessions = storage.list_sessions().await.unwrap();

    assert_eq!(sessions[0].id, first.id);
    assert_eq!(sessions[1].id, second.id);
}

#[tokio::test]
async fn delete_session_removes_messages() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let session = storage.create_session("Delete me").await.unwrap();
    storage.append_turn(&session.id, "user", "assistant").await.unwrap();

    storage.delete_session(&session.id).await.unwrap();

    assert!(!storage.session_exists(&session.id).await.unwrap());
    assert!(storage.list_messages(&session.id).await.unwrap().is_empty());
}
```

- [ ] **Step 2: Run storage tests to verify they fail**

Run: `pixi run test -p agent-runtime storage::tests::list_sessions_returns_newest_updated_first storage::tests::delete_session_removes_messages`

Expected: FAIL because `list_sessions` and `delete_session` do not exist.

- [ ] **Step 3: Implement storage helpers**

Add these methods to `impl Storage` in `crates/agent-runtime/src/storage.rs`:

```rust
pub async fn list_sessions(&self) -> anyhow::Result<Vec<Session>> {
    let rows = sqlx::query(
        "SELECT id, title, created_at, updated_at FROM sessions ORDER BY updated_at DESC, created_at DESC, id ASC",
    )
    .fetch_all(&self.pool)
    .await?;

    let mut sessions = Vec::with_capacity(rows.len());
    for row in rows {
        let created_at: String = row.try_get("created_at")?;
        let updated_at: String = row.try_get("updated_at")?;
        sessions.push(Session {
            id: row.try_get("id")?,
            title: row.try_get("title")?,
            created_at: DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc),
            updated_at: DateTime::parse_from_rfc3339(&updated_at)?.with_timezone(&Utc),
        });
    }
    Ok(sessions)
}

pub async fn delete_session(&self, session_id: &str) -> anyhow::Result<()> {
    let mut tx = self.pool.begin().await?;
    sqlx::query("DELETE FROM messages WHERE session_id = ?")
        .bind(session_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM sessions WHERE id = ?")
        .bind(session_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
```

- [ ] **Step 4: Add model config types**

Create `crates/agent-runtime/src/model_config.rs`:

```rust
use model_gateway::provider::EndpointType;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct StoredModelConfig {
    pub provider_id: String,
    pub provider_name: String,
    pub endpoint_type: EndpointType,
    pub base_url: String,
    pub model_name: String,
    pub secret_id: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

impl StoredModelConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.base_url.trim().is_empty() {
            return Err("base URL is required".into());
        }
        if self.model_name.trim().is_empty() {
            return Err("model name is required".into());
        }
        Ok(())
    }
}
```

Modify `crates/agent-runtime/src/lib.rs`:

```rust
pub mod model_config;
```

- [ ] **Step 5: Write mobile host tests**

Create `crates/agent-runtime/src/mobile_host.rs` with the tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{CapabilitySet, PlatformId};
    use crate::skill::SkillRegistry;
    use crate::skill_catalog::SkillCatalog;
    use crate::storage::Storage;
    use crate::tools::RuntimeConfig;
    use futures::stream;
    use model_gateway::responses::GatewayEvent;
    use tempfile::tempdir;

    struct FakeModel;

    #[async_trait::async_trait]
    impl crate::turn::ModelClient for FakeModel {
        async fn stream(&self, _request: model_gateway::responses::GatewayRequest) -> anyhow::Result<crate::turn::ModelEventStream> {
            Ok(Box::pin(stream::iter(vec![
                Ok(GatewayEvent::TextDelta { text: "hello from android".into() }),
            ])))
        }
    }

    #[tokio::test]
    async fn mobile_host_persists_turn_messages() {
        let dir = tempdir().unwrap();
        let db_url = format!("sqlite://{}?mode=rwc", dir.path().join("ga.db").display());
        let storage = Storage::connect(&db_url).await.unwrap();
        let runtime_config = RuntimeConfig::workspace_write(dir.path(), dir.path()).without_builtin_tools();
        let host = MobileRuntimeHost::new_for_test(
            storage,
            FakeModel,
            SkillRegistry::empty(),
            SkillCatalog::empty(),
            runtime_config,
            MobileRuntimeInit {
                platform: PlatformId::Android,
                capabilities: CapabilitySet::android_mvp(),
            },
        );

        let session = host.create_session("Mobile").await.unwrap();
        let result = host.send_message(&session.id, "Hi").await.unwrap();
        let messages = host.get_messages(&session.id).await.unwrap();

        assert_eq!(result.assistant_text, "hello from android");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[1].content, "hello from android");
    }
}
```

- [ ] **Step 6: Run mobile host tests to verify they fail**

Run: `pixi run test -p agent-runtime mobile_host`

Expected: FAIL because `MobileRuntimeHost` does not exist and `SkillRegistry::empty()` may need to be added.

- [ ] **Step 7: Implement mobile host**

Add this implementation above the test module in `crates/agent-runtime/src/mobile_host.rs`:

```rust
use crate::events::RuntimeEvent;
use crate::platform::{CapabilitySet, PlatformId};
use crate::session::{Message, Session};
use crate::skill::SkillRegistry;
use crate::skill_catalog::SkillCatalog;
use crate::storage::Storage;
use crate::tools::RuntimeConfig;
use crate::turn::{ModelClient, TurnRunner};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct MobileRuntimeInit {
    pub platform: PlatformId,
    pub capabilities: CapabilitySet,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct MobileTurnResult {
    pub assistant_text: String,
    pub events: Vec<RuntimeEvent>,
}

pub struct MobileRuntimeHost<C> {
    storage: Storage,
    model: C,
    skills: SkillRegistry,
    skill_catalog: SkillCatalog,
    runtime_config: RuntimeConfig,
    init: MobileRuntimeInit,
}

impl<C> MobileRuntimeHost<C>
where
    C: ModelClient,
{
    pub fn new_for_test(
        storage: Storage,
        model: C,
        skills: SkillRegistry,
        skill_catalog: SkillCatalog,
        runtime_config: RuntimeConfig,
        init: MobileRuntimeInit,
    ) -> Self {
        Self {
            storage,
            model,
            skills,
            skill_catalog,
            runtime_config,
            init,
        }
    }

    pub async fn create_session(&self, title: &str) -> anyhow::Result<Session> {
        self.storage.create_session(title).await
    }

    pub async fn list_sessions(&self) -> anyhow::Result<Vec<Session>> {
        self.storage.list_sessions().await
    }

    pub async fn get_messages(&self, session_id: &str) -> anyhow::Result<Vec<Message>> {
        self.storage.list_messages(session_id).await
    }

    pub async fn delete_session(&self, session_id: &str) -> anyhow::Result<()> {
        self.storage.delete_session(session_id).await
    }

    pub fn init(&self) -> &MobileRuntimeInit {
        &self.init
    }
}

impl<C> MobileRuntimeHost<C>
where
    C: ModelClient + Clone,
{
    pub async fn send_message(
        &self,
        session_id: &str,
        content: &str,
    ) -> anyhow::Result<MobileTurnResult> {
        if !self.storage.session_exists(session_id).await? {
            anyhow::bail!("session not found");
        }
        let runner = TurnRunner::new_with_catalog_and_config(
            self.model.clone(),
            self.skills.clone(),
            self.skill_catalog.clone(),
            self.runtime_config.clone(),
        );
        let events = runner.run(content).await?;
        let assistant_text = events.iter().find_map(|event| {
            if let RuntimeEvent::AssistantMessageFinished { text } = event {
                Some(text.clone())
            } else {
                None
            }
        }).unwrap_or_else(|| {
            events.iter().filter_map(|event| {
                if let RuntimeEvent::AssistantTextDelta { text } = event {
                    Some(text.as_str())
                } else {
                    None
                }
            }).collect::<String>()
        });
        self.storage
            .append_turn(session_id, content, &assistant_text)
            .await?;
        Ok(MobileTurnResult {
            assistant_text,
            events,
        })
    }
}
```

Add a focused helper to `crates/agent-runtime/src/skill.rs` if it does not already exist:

```rust
impl SkillRegistry {
    pub fn empty() -> Self {
        Self::default()
    }
}
```

Modify `crates/agent-runtime/src/lib.rs`:

```rust
pub mod mobile_host;
```

- [ ] **Step 8: Add missing dev dependency**

Modify `crates/agent-runtime/Cargo.toml`:

```toml
[dev-dependencies]
tempfile = "3"
```

If `[dev-dependencies]` already exists, add only `tempfile = "3"` inside it.

- [ ] **Step 9: Run task tests**

Run: `pixi run test -p agent-runtime storage mobile_host model_config`

Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add crates/agent-runtime/Cargo.toml crates/agent-runtime/src/lib.rs crates/agent-runtime/src/mobile_host.rs crates/agent-runtime/src/model_config.rs crates/agent-runtime/src/skill.rs crates/agent-runtime/src/storage.rs Cargo.lock
git commit -m "feat: add mobile runtime host"
```

### Task 5: HTTP Model Config and Secret Bridge

**Files:**
- Modify: `crates/agent-runtime/src/model_config.rs`
- Modify: `crates/agent-runtime/src/mobile_host.rs`
- Test: `crates/agent-runtime/src/model_config.rs`
- Test: `crates/agent-runtime/src/mobile_host.rs`

**Interfaces:**
- Consumes: `StoredModelConfig`, `MobileRuntimeHost` from Task 4
- Produces: `SecretResolver`, `StoredModelConfig::to_provider_profile(...)`, `HttpMobileRuntimeHost`

- [ ] **Step 1: Add model config tests**

Append these tests to `crates/agent-runtime/src/model_config.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use model_gateway::provider::EndpointType;
    use std::collections::BTreeMap;

    #[test]
    fn provider_profile_uses_runtime_secret_without_persisting_it() {
        let config = StoredModelConfig {
            provider_id: "openai".into(),
            provider_name: "OpenAI".into(),
            endpoint_type: EndpointType::Responses,
            base_url: "https://api.openai.com/v1".into(),
            model_name: "gpt-5.4".into(),
            secret_id: Some("model.openai.default".into()),
            headers: BTreeMap::new(),
        };

        let profile = config.to_provider_profile(Some("sk-runtime".into()));
        let stored_json = serde_json::to_string(&config).unwrap();

        assert_eq!(profile.api_key.as_deref(), Some("sk-runtime"));
        assert_eq!(profile.model, "gpt-5.4");
        assert!(!stored_json.contains("sk-runtime"));
    }

    #[test]
    fn validates_required_model_fields() {
        let config = StoredModelConfig {
            provider_id: "local".into(),
            provider_name: "Local".into(),
            endpoint_type: EndpointType::ChatCompletions,
            base_url: "".into(),
            model_name: "".into(),
            secret_id: None,
            headers: BTreeMap::new(),
        };

        assert_eq!(config.validate().unwrap_err(), "base URL is required");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `pixi run test -p agent-runtime model_config`

Expected: FAIL because `to_provider_profile` does not exist.

- [ ] **Step 3: Implement provider conversion**

Modify `crates/agent-runtime/src/model_config.rs`:

```rust
use model_gateway::provider::{EndpointType, ProviderProfile};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct StoredModelConfig {
    pub provider_id: String,
    pub provider_name: String,
    pub endpoint_type: EndpointType,
    pub base_url: String,
    pub model_name: String,
    pub secret_id: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

impl StoredModelConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.base_url.trim().is_empty() {
            return Err("base URL is required".into());
        }
        if self.model_name.trim().is_empty() {
            return Err("model name is required".into());
        }
        Ok(())
    }

    pub fn to_provider_profile(&self, api_key: Option<String>) -> ProviderProfile {
        ProviderProfile {
            id: self.provider_id.clone(),
            name: self.provider_name.clone(),
            endpoint_type: self.endpoint_type,
            base_url: self.base_url.clone(),
            model: self.model_name.clone(),
            api_key,
            headers: self.headers.clone(),
        }
    }
}
```

- [ ] **Step 4: Add secret resolver and HTTP mobile host tests**

Append this test to `crates/agent-runtime/src/mobile_host.rs`:

```rust
#[cfg(test)]
mod http_tests {
    use super::*;
    use crate::model_config::StoredModelConfig;
    use model_gateway::provider::EndpointType;
    use std::collections::BTreeMap;

    struct StaticSecretResolver;

    #[async_trait::async_trait]
    impl SecretResolver for StaticSecretResolver {
        async fn resolve_secret(&self, secret_id: &str) -> anyhow::Result<Option<String>> {
            assert_eq!(secret_id, "model.openai.default");
            Ok(Some("sk-runtime".into()))
        }
    }

    #[tokio::test]
    async fn resolves_model_secret_for_provider_profile() {
        let model_config = StoredModelConfig {
            provider_id: "openai".into(),
            provider_name: "OpenAI".into(),
            endpoint_type: EndpointType::Responses,
            base_url: "https://api.openai.com/v1".into(),
            model_name: "gpt-5.4".into(),
            secret_id: Some("model.openai.default".into()),
            headers: BTreeMap::new(),
        };

        let api_key = resolve_model_api_key(&model_config, &StaticSecretResolver)
            .await
            .unwrap();

        assert_eq!(api_key.as_deref(), Some("sk-runtime"));
    }
}
```

- [ ] **Step 5: Run tests to verify they fail**

Run: `pixi run test -p agent-runtime http_tests`

Expected: FAIL because `SecretResolver` and `resolve_model_api_key` do not exist.

- [ ] **Step 6: Implement secret bridge helpers**

Add to `crates/agent-runtime/src/mobile_host.rs` near the existing mobile host types:

```rust
#[async_trait::async_trait]
pub trait SecretResolver: Send + Sync {
    async fn resolve_secret(&self, secret_id: &str) -> anyhow::Result<Option<String>>;
}

pub async fn resolve_model_api_key<R>(
    model_config: &crate::model_config::StoredModelConfig,
    resolver: &R,
) -> anyhow::Result<Option<String>>
where
    R: SecretResolver,
{
    if let Some(secret_id) = &model_config.secret_id {
        return resolver.resolve_secret(secret_id).await;
    }
    Ok(None)
}
```

Add an HTTP host wrapper to the same file:

```rust
pub struct HttpMobileRuntimeHost<R> {
    storage: Storage,
    skills: SkillRegistry,
    skill_catalog: SkillCatalog,
    runtime_config: RuntimeConfig,
    model_config: crate::model_config::StoredModelConfig,
    secret_resolver: R,
}

impl<R> HttpMobileRuntimeHost<R>
where
    R: SecretResolver,
{
    pub fn new(
        storage: Storage,
        skills: SkillRegistry,
        skill_catalog: SkillCatalog,
        runtime_config: RuntimeConfig,
        model_config: crate::model_config::StoredModelConfig,
        secret_resolver: R,
    ) -> Self {
        Self {
            storage,
            skills,
            skill_catalog,
            runtime_config,
            model_config,
            secret_resolver,
        }
    }

    pub async fn send_message(
        &self,
        session_id: &str,
        content: &str,
    ) -> anyhow::Result<MobileTurnResult> {
        if !self.storage.session_exists(session_id).await? {
            anyhow::bail!("session not found");
        }
        self.model_config
            .validate()
            .map_err(|message| anyhow::anyhow!(message))?;
        let api_key = resolve_model_api_key(&self.model_config, &self.secret_resolver).await?;
        let profile = self.model_config.to_provider_profile(api_key);
        let runner = TurnRunner::new_with_catalog_and_config(
            model_gateway::responses::GatewayHttpClient::new(profile),
            self.skills.clone(),
            self.skill_catalog.clone(),
            self.runtime_config.clone(),
        );
        let events = runner.run(content).await?;
        let assistant_text = assistant_text_from_events(&events);
        self.storage
            .append_turn(session_id, content, &assistant_text)
            .await?;
        Ok(MobileTurnResult {
            assistant_text,
            events,
        })
    }
}

fn assistant_text_from_events(events: &[RuntimeEvent]) -> String {
    events
        .iter()
        .find_map(|event| {
            if let RuntimeEvent::AssistantMessageFinished { text } = event {
                Some(text.clone())
            } else {
                None
            }
        })
        .unwrap_or_else(|| {
            events
                .iter()
                .filter_map(|event| {
                    if let RuntimeEvent::AssistantTextDelta { text } = event {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .collect::<String>()
        })
}
```

Replace the duplicated assistant-text extraction in `MobileRuntimeHost::send_message` with:

```rust
let assistant_text = assistant_text_from_events(&events);
```

- [ ] **Step 7: Run task tests**

Run: `pixi run test -p agent-runtime model_config mobile_host`

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/agent-runtime/src/model_config.rs crates/agent-runtime/src/mobile_host.rs
git commit -m "feat: add mobile http model bridge"
```

### Task 6: Mobile FFI Facade

**Files:**
- Create: `crates/mobile-ffi/Cargo.toml`
- Create: `crates/mobile-ffi/src/lib.rs`
- Create: `crates/mobile-ffi/src/types.rs`
- Create: `crates/mobile-ffi/src/runtime.rs`
- Create: `crates/mobile-ffi/tests/mobile_runtime.rs`
- Modify: `Cargo.toml`
- Modify: `pixi.toml`

**Interfaces:**
- Consumes: `MobileRuntimeHost` from Task 4 and HTTP model bridge from Task 5
- Produces: `MobileRuntime`, `MobileInitConfig`, `MobileDiagnostics`, `MobileSessionDto`, `MobileMessageDto`, `MobileSkillDto`, `MobileTurnDto`

- [ ] **Step 1: Add crate skeleton and failing facade test**

Modify root `Cargo.toml` members:

```toml
members = [
  "crates/model-gateway",
  "crates/agent-runtime",
  "crates/agent-server",
  "crates/mobile-ffi",
]
```

Create `crates/mobile-ffi/Cargo.toml`:

```toml
[package]
name = "mobile-ffi"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
agent-runtime = { path = "../agent-runtime" }
anyhow.workspace = true
model-gateway = { path = "../model-gateway" }
serde.workspace = true
tokio.workspace = true
uuid.workspace = true

[dev-dependencies]
tempfile = "3"
```

Create `crates/mobile-ffi/tests/mobile_runtime.rs`:

```rust
use mobile_ffi::{MobileInitConfig, MobileRuntime};
use tempfile::tempdir;

#[test]
fn initializes_runtime_and_returns_android_capabilities() {
    let dir = tempdir().unwrap();
    let runtime = MobileRuntime::initialize(MobileInitConfig {
        app_data_dir: dir.path().join("files").display().to_string(),
        cache_dir: dir.path().join("cache").display().to_string(),
        database_path: dir.path().join("general-agent.db").display().to_string(),
        skills_dir: "skills".into(),
        platform: "android".into(),
        capabilities: vec![
            "network.http".into(),
            "filesystem.app_data".into(),
            "secure_storage".into(),
            "model.http_provider".into(),
        ],
    }).unwrap();

    let diagnostics = runtime.diagnostics();
    assert_eq!(diagnostics.platform, "android");
    assert!(diagnostics.capabilities.contains(&"network.http".into()));
    assert!(diagnostics.database_ready);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pixi run test -p mobile-ffi`

Expected: FAIL because `MobileRuntime` does not exist.

- [ ] **Step 3: Add FFI-safe DTOs**

Create `crates/mobile-ffi/src/types.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileInitConfig {
    pub app_data_dir: String,
    pub cache_dir: String,
    pub database_path: String,
    pub skills_dir: String,
    pub platform: String,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileDiagnostics {
    pub platform: String,
    pub capabilities: Vec<String>,
    pub database_ready: bool,
    pub skills_ready: bool,
    pub model_configured: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileSessionDto {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileMessageDto {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileTurnDto {
    pub assistant_text: String,
}
```

- [ ] **Step 4: Add runtime facade**

Create `crates/mobile-ffi/src/runtime.rs`:

```rust
use crate::types::*;
use agent_runtime::platform::{CapabilitySet, PlatformId};
use agent_runtime::storage::Storage;
use std::path::PathBuf;
use tokio::runtime::Runtime;

pub struct MobileRuntime {
    tokio: Runtime,
    platform: PlatformId,
    capabilities: CapabilitySet,
    database_ready: bool,
}

impl MobileRuntime {
    pub fn initialize(config: MobileInitConfig) -> anyhow::Result<Self> {
        let tokio = Runtime::new()?;
        let platform = match config.platform.as_str() {
            "android" => PlatformId::Android,
            "desktop" => PlatformId::Desktop,
            "ios" => PlatformId::Ios,
            "web" => PlatformId::Web,
            "server" => PlatformId::Server,
            _ => anyhow::bail!("unsupported platform: {}", config.platform),
        };
        let capabilities = CapabilitySet::from_names(config.capabilities);
        let database_path = PathBuf::from(config.database_path);
        if let Some(parent) = database_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let database_url = format!("sqlite://{}?mode=rwc", database_path.display());
        tokio.block_on(Storage::connect(&database_url))?;
        Ok(Self {
            tokio,
            platform,
            capabilities,
            database_ready: true,
        })
    }

    pub fn diagnostics(&self) -> MobileDiagnostics {
        MobileDiagnostics {
            platform: match self.platform {
                PlatformId::Android => "android",
                PlatformId::Desktop => "desktop",
                PlatformId::Ios => "ios",
                PlatformId::Web => "web",
                PlatformId::Server => "server",
            }
            .into(),
            capabilities: self.capabilities.names().to_vec(),
            database_ready: self.database_ready,
            skills_ready: false,
            model_configured: false,
        }
    }
}
```

Create `crates/mobile-ffi/src/lib.rs`:

```rust
pub mod runtime;
pub mod types;

pub use runtime::MobileRuntime;
pub use types::{
    MobileDiagnostics, MobileInitConfig, MobileMessageDto, MobileSessionDto, MobileTurnDto,
};
```

- [ ] **Step 5: Add pixi task**

Modify `pixi.toml`:

```toml
mobile-ffi-test = "cargo test -p mobile-ffi"
```

Place it under `[tasks]`.

- [ ] **Step 6: Run facade tests**

Run: `pixi run mobile-ffi-test`

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock pixi.toml crates/mobile-ffi
git commit -m "feat: add mobile ffi facade"
```

### Task 7: Android Scaffold and Native Bridge

**Files:**
- Create: `apps/android/settings.gradle.kts`
- Create: `apps/android/build.gradle.kts`
- Create: `apps/android/gradle/libs.versions.toml`
- Create: `apps/android/app/build.gradle.kts`
- Create: `apps/android/app/src/main/AndroidManifest.xml`
- Create: `apps/android/app/src/main/java/com/generalagent/mobile/MainActivity.kt`
- Create: `apps/android/app/src/main/java/com/generalagent/mobile/runtime/AndroidCapabilities.kt`
- Create: `apps/android/app/src/main/java/com/generalagent/mobile/runtime/RuntimeBridge.kt`
- Create: `apps/android/app/src/test/java/com/generalagent/mobile/runtime/AndroidCapabilitiesTest.kt`
- Modify: `pixi.toml`

**Interfaces:**
- Consumes: `MobileRuntime` facade from Task 6
- Produces: Android app module that initializes the runtime bridge with Android MVP capabilities

- [ ] **Step 1: Create Gradle settings**

Create `apps/android/settings.gradle.kts`:

```kotlin
pluginManagement {
  repositories {
    google()
    mavenCentral()
    gradlePluginPortal()
  }
}

dependencyResolutionManagement {
  repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
  repositories {
    google()
    mavenCentral()
  }
}

rootProject.name = "GeneralAgentAndroid"
include(":app")
```

Create `apps/android/build.gradle.kts`:

```kotlin
plugins {
  alias(libs.plugins.android.application) apply false
  alias(libs.plugins.kotlin.android) apply false
  alias(libs.plugins.kotlin.compose) apply false
}
```

Create `apps/android/gradle/libs.versions.toml`:

```toml
[versions]
agp = "9.2.1"
androidx-activity = "1.13.0"
androidx-compose-bom = "2026.06.01"
androidx-core = "1.19.0"
androidx-lifecycle = "2.11.0"
junit = "4.13.2"
kotlin = "2.4.0"
robolectric = "4.16.1"

[libraries]
androidx-activity-compose = { module = "androidx.activity:activity-compose", version.ref = "androidx-activity" }
androidx-compose-bom = { module = "androidx.compose:compose-bom", version.ref = "androidx-compose-bom" }
androidx-compose-material3 = { module = "androidx.compose.material3:material3" }
androidx-compose-ui = { module = "androidx.compose.ui:ui" }
androidx-compose-ui-tooling = { module = "androidx.compose.ui:ui-tooling" }
androidx-compose-ui-tooling-preview = { module = "androidx.compose.ui:ui-tooling-preview" }
androidx-core-ktx = { module = "androidx.core:core-ktx", version.ref = "androidx-core" }
androidx-lifecycle-runtime-ktx = { module = "androidx.lifecycle:lifecycle-runtime-ktx", version.ref = "androidx-lifecycle" }
junit = { module = "junit:junit", version.ref = "junit" }
robolectric = { module = "org.robolectric:robolectric", version.ref = "robolectric" }

[plugins]
android-application = { id = "com.android.application", version.ref = "agp" }
kotlin-android = { id = "org.jetbrains.kotlin.android", version.ref = "kotlin" }
kotlin-compose = { id = "org.jetbrains.kotlin.plugin.compose", version.ref = "kotlin" }
```

- [ ] **Step 2: Add app module**

Create `apps/android/app/build.gradle.kts`:

```kotlin
plugins {
  alias(libs.plugins.android.application)
  alias(libs.plugins.kotlin.android)
  alias(libs.plugins.kotlin.compose)
}

android {
  namespace = "com.generalagent.mobile"
  compileSdk = 37

  defaultConfig {
    applicationId = "com.generalagent.mobile"
    minSdk = 31
    targetSdk = 36
    versionCode = 1
    versionName = "0.1.0"
  }

  buildFeatures {
    compose = true
  }

  compileOptions {
    sourceCompatibility = JavaVersion.VERSION_17
    targetCompatibility = JavaVersion.VERSION_17
  }

  testOptions {
    unitTests.isIncludeAndroidResources = true
  }
}

kotlin {
  compilerOptions {
    jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
  }
}

dependencies {
  val composeBom = platform(libs.androidx.compose.bom)
  implementation(composeBom)
  testImplementation(libs.junit)
  testImplementation(libs.robolectric)

  implementation(libs.androidx.activity.compose)
  implementation(libs.androidx.core.ktx)
  implementation(libs.androidx.lifecycle.runtime.ktx)
  implementation(libs.androidx.compose.material3)
  implementation(libs.androidx.compose.ui)
  debugImplementation(libs.androidx.compose.ui.tooling)
  implementation(libs.androidx.compose.ui.tooling.preview)
}
```

Create `apps/android/app/src/main/AndroidManifest.xml`:

```xml
<manifest xmlns:android="http://schemas.android.com/apk/res/android">
    <uses-permission android:name="android.permission.INTERNET" />

    <application
        android:allowBackup="false"
        android:label="GeneralAgent"
        android:theme="@style/Theme.GeneralAgent">
        <activity
            android:name=".MainActivity"
            android:exported="true">
            <intent-filter>
                <action android:name="android.intent.action.MAIN" />
                <category android:name="android.intent.category.LAUNCHER" />
            </intent-filter>
        </activity>
    </application>
</manifest>
```

Create `apps/android/app/src/main/res/values/themes.xml`:

```xml
<resources>
    <style name="Theme.GeneralAgent" parent="android:style/Theme.Material.Light.NoActionBar" />
</resources>
```

- [ ] **Step 3: Add Android capability test**

Create `apps/android/app/src/test/java/com/generalagent/mobile/runtime/AndroidCapabilitiesTest.kt`:

```kotlin
package com.generalagent.mobile.runtime

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class AndroidCapabilitiesTest {
  @Test
  fun androidMvpCapabilitiesContainOnlyMobileSafeCoreCapabilities() {
    val capabilities = androidMvpCapabilities()

    assertEquals(
      listOf("network.http", "filesystem.app_data", "secure_storage", "model.http_provider"),
      capabilities,
    )
    assertTrue(capabilities.contains("network.http"))
    assertFalse(capabilities.contains("shell.process"))
    assertFalse(capabilities.contains("browser.headless"))
  }
}
```

- [ ] **Step 4: Run Android unit test to verify it fails**

Run: `cd apps/android && ./gradlew :app:testDebugUnitTest --tests com.generalagent.mobile.runtime.AndroidCapabilitiesTest`

Expected: FAIL because `androidMvpCapabilities` does not exist.

- [ ] **Step 5: Add runtime bridge and main activity**

Create `apps/android/app/src/main/java/com/generalagent/mobile/runtime/AndroidCapabilities.kt`:

```kotlin
package com.generalagent.mobile.runtime

fun androidMvpCapabilities(): List<String> =
  listOf(
    "network.http",
    "filesystem.app_data",
    "secure_storage",
    "model.http_provider",
  )
```

Create `apps/android/app/src/main/java/com/generalagent/mobile/runtime/RuntimeBridge.kt`:

```kotlin
package com.generalagent.mobile.runtime

import android.content.Context

data class RuntimeInitRequest(
  val appDataDir: String,
  val cacheDir: String,
  val databasePath: String,
  val skillsDir: String,
  val platform: String = "android",
  val capabilities: List<String> = androidMvpCapabilities(),
)

data class RuntimeDiagnostics(
  val platform: String,
  val capabilities: List<String>,
  val databaseReady: Boolean,
  val skillsReady: Boolean,
  val modelConfigured: Boolean,
)

interface RuntimeClient {
  fun diagnostics(): RuntimeDiagnostics
}

class RuntimeBridge(
  private val context: Context,
  private val loader: NativeRuntimeLoader = NativeRuntimeLoader,
) {
  fun initRequest(): RuntimeInitRequest {
    val filesDir = context.filesDir
    val cacheDir = context.cacheDir
    return RuntimeInitRequest(
      appDataDir = filesDir.absolutePath,
      cacheDir = cacheDir.absolutePath,
      databasePath = filesDir.resolve("general-agent.db").absolutePath,
      skillsDir = filesDir.resolve("skills").absolutePath,
    )
  }

  fun load(): RuntimeClient = loader.load(initRequest())
}

object NativeRuntimeLoader {
  fun load(request: RuntimeInitRequest): RuntimeClient =
    StaticDiagnosticsRuntimeClient(request)
}

private class StaticDiagnosticsRuntimeClient(
  private val request: RuntimeInitRequest,
) : RuntimeClient {
  override fun diagnostics(): RuntimeDiagnostics =
    RuntimeDiagnostics(
      platform = request.platform,
      capabilities = request.capabilities,
      databaseReady = true,
      skillsReady = false,
      modelConfigured = false,
    )
}
```

Create `apps/android/app/src/main/java/com/generalagent/mobile/MainActivity.kt`:

```kotlin
package com.generalagent.mobile

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import com.generalagent.mobile.runtime.RuntimeBridge

class MainActivity : ComponentActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    val diagnostics = RuntimeBridge(this).load().diagnostics()
    setContent {
      MaterialTheme {
        Text("GeneralAgent ${diagnostics.platform}")
      }
    }
  }
}
```

- [ ] **Step 6: Add pixi Android tasks**

Modify `pixi.toml`:

```toml
android-test = "cd apps/android && ./gradlew :app:testDebugUnitTest"
android-assemble = "cd apps/android && ./gradlew :app:assembleDebug"
```

- [ ] **Step 7: Run Android unit tests**

Run: `pixi run android-test`

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add apps/android pixi.toml
git commit -m "feat: scaffold android app"
```

### Task 8: Android Keystore Model Settings

**Files:**
- Create: `apps/android/app/src/main/java/com/generalagent/mobile/secrets/ModelSecretStore.kt`
- Create: `apps/android/app/src/main/java/com/generalagent/mobile/model/ModelSettings.kt`
- Create: `apps/android/app/src/test/java/com/generalagent/mobile/secrets/ModelSecretStoreTest.kt`
- Create: `apps/android/app/src/test/java/com/generalagent/mobile/model/ModelSettingsTest.kt`

**Interfaces:**
- Consumes: Android scaffold from Task 7
- Produces: `ModelSecretStore`, `ModelSettings`, `ModelSettings.redactedForRust()`

- [ ] **Step 1: Add tests**

Create `apps/android/app/src/test/java/com/generalagent/mobile/model/ModelSettingsTest.kt`:

```kotlin
package com.generalagent.mobile.model

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test

class ModelSettingsTest {
  @Test
  fun redactedSettingsNeverContainApiKey() {
    val settings =
      ModelSettings(
        providerId = "openai",
        providerName = "OpenAI",
        endpointType = "responses",
        baseUrl = "https://api.openai.com/v1",
        modelName = "gpt-5.4",
        secretId = "model.openai.default",
        apiKey = "sk-secret",
      )

    val redacted = settings.redactedForRust()

    assertEquals("model.openai.default", redacted.secretId)
    assertFalse(redacted.toString().contains("sk-secret"))
  }
}
```

Create `apps/android/app/src/test/java/com/generalagent/mobile/secrets/ModelSecretStoreTest.kt`:

```kotlin
package com.generalagent.mobile.secrets

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class ModelSecretStoreTest {
  @Test
  fun inMemoryStoreSavesLoadsAndDeletesSecret() {
    val store = InMemoryModelSecretStore()

    store.saveSecret("model.default", "sk-test")

    assertEquals("sk-test", store.loadSecret("model.default"))
    store.deleteSecret("model.default")
    assertNull(store.loadSecret("model.default"))
  }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `pixi run android-test -- --tests com.generalagent.mobile.model.ModelSettingsTest --tests com.generalagent.mobile.secrets.ModelSecretStoreTest`

Expected: FAIL because model and secret classes do not exist.

- [ ] **Step 3: Add model settings types**

Create `apps/android/app/src/main/java/com/generalagent/mobile/model/ModelSettings.kt`:

```kotlin
package com.generalagent.mobile.model

import com.generalagent.mobile.runtime.RuntimeModelConfig

data class ModelSettings(
  val providerId: String,
  val providerName: String,
  val endpointType: String,
  val baseUrl: String,
  val modelName: String,
  val secretId: String?,
  val apiKey: String?,
) {
  fun redactedForRust(): RuntimeModelConfig =
    RuntimeModelConfig(
      providerId = providerId,
      providerName = providerName,
      endpointType = endpointType,
      baseUrl = baseUrl,
      modelName = modelName,
      secretId = secretId,
    )
}
```

`RuntimeModelConfig` intentionally has no API-key field, so plaintext secrets cannot enter the Rust persistence DTO by construction.

- [ ] **Step 4: Add secret store interface and test implementation**

Create `apps/android/app/src/main/java/com/generalagent/mobile/secrets/ModelSecretStore.kt`:

```kotlin
package com.generalagent.mobile.secrets

interface ModelSecretStore {
  fun saveSecret(secretId: String, value: String)
  fun loadSecret(secretId: String): String?
  fun deleteSecret(secretId: String)
}

class InMemoryModelSecretStore : ModelSecretStore {
  private val values = linkedMapOf<String, String>()

  override fun saveSecret(secretId: String, value: String) {
    values[secretId] = value
  }

  override fun loadSecret(secretId: String): String? = values[secretId]

  override fun deleteSecret(secretId: String) {
    values.remove(secretId)
  }
}
```

- [ ] **Step 5: Add Android Keystore implementation**

Implement `AndroidKeystoreModelSecretStore` with platform APIs directly:

- Generate or load a non-exportable AES-256 key from the `AndroidKeyStore` provider.
- Encrypt each value with `AES/GCM/NoPadding` and a fresh random IV.
- Bind ciphertext to its secret ID with GCM additional authenticated data.
- Hash secret IDs before using them as filenames.
- Store versioned ciphertext envelopes under `Context.noBackupFilesDir`.
- Replace files atomically and reject malformed or tampered ciphertext.
- Do not add `androidx.security:security-crypto`; its crypto APIs are deprecated in favor of direct platform APIs.

- [ ] **Step 6: Run tests**

Run: `pixi run android-test`

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add apps/android
git commit -m "feat: add android model secret storage"
```

### Task 9: Compose MVP Screens

**Files:**
- Create: `apps/android/app/src/main/java/com/generalagent/mobile/ui/AppRoot.kt`
- Create: `apps/android/app/src/main/java/com/generalagent/mobile/ui/ChatScreen.kt`
- Create: `apps/android/app/src/main/java/com/generalagent/mobile/ui/SettingsScreen.kt`
- Create: `apps/android/app/src/main/java/com/generalagent/mobile/ui/SkillsScreen.kt`
- Create: `apps/android/app/src/main/java/com/generalagent/mobile/ui/DiagnosticsScreen.kt`
- Modify: `apps/android/app/src/main/java/com/generalagent/mobile/MainActivity.kt`
- Test: `apps/android/app/src/test/java/com/generalagent/mobile/ui/AppRootStateTest.kt`

**Interfaces:**
- Consumes: `RuntimeBridge`, `RuntimeDiagnostics`, `ModelSettings`
- Produces: four-screen Compose MVP with Chat, Settings, Skills, Diagnostics

- [ ] **Step 1: Add UI state test**

Create `apps/android/app/src/test/java/com/generalagent/mobile/ui/AppRootStateTest.kt`:

```kotlin
package com.generalagent.mobile.ui

import org.junit.Assert.assertEquals
import org.junit.Test

class AppRootStateTest {
  @Test
  fun tabsExposeMvpScreensInStableOrder() {
    assertEquals(
      listOf(AppTab.Chat, AppTab.Settings, AppTab.Skills, AppTab.Diagnostics),
      AppTab.entries,
    )
  }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pixi run android-test -- --tests com.generalagent.mobile.ui.AppRootStateTest`

Expected: FAIL because `AppTab` does not exist.

- [ ] **Step 3: Add tab root**

Create `apps/android/app/src/main/java/com/generalagent/mobile/ui/AppRoot.kt`:

```kotlin
package com.generalagent.mobile.ui

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import com.generalagent.mobile.runtime.RuntimeDiagnostics

enum class AppTab(val label: String) {
  Chat("Chat"),
  Settings("Settings"),
  Skills("Skills"),
  Diagnostics("Diagnostics"),
}

@Composable
fun AppRoot(
  diagnostics: RuntimeDiagnostics,
  modifier: Modifier = Modifier,
) {
  var selected by remember { mutableStateOf(AppTab.Chat) }
  Scaffold(
    modifier = modifier.fillMaxSize(),
    bottomBar = {
      NavigationBar {
        AppTab.entries.forEach { tab ->
          NavigationBarItem(
            selected = selected == tab,
            onClick = { selected = tab },
            label = { Text(tab.label) },
            icon = { Text(tab.label.first().toString()) },
          )
        }
      }
    },
  ) { padding ->
    Column {
      when (selected) {
        AppTab.Chat -> ChatScreen()
        AppTab.Settings -> SettingsScreen()
        AppTab.Skills -> SkillsScreen()
        AppTab.Diagnostics -> DiagnosticsScreen(diagnostics)
      }
    }
  }
}
```

- [ ] **Step 4: Add initial screen content**

Create `apps/android/app/src/main/java/com/generalagent/mobile/ui/ChatScreen.kt`:

```kotlin
package com.generalagent.mobile.ui

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp

@Composable
fun ChatScreen() {
  var draft by remember { mutableStateOf("") }
  Column(modifier = Modifier.padding(16.dp)) {
    Text("GeneralAgent")
    OutlinedTextField(
      value = draft,
      onValueChange = { draft = it },
      label = { Text("Message") },
    )
    Button(onClick = { draft = "" }, enabled = draft.isNotBlank()) {
      Text("Send")
    }
  }
}
```

Create `apps/android/app/src/main/java/com/generalagent/mobile/ui/SettingsScreen.kt`:

```kotlin
package com.generalagent.mobile.ui

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp

@Composable
fun SettingsScreen() {
  var baseUrl by remember { mutableStateOf("https://api.openai.com/v1") }
  var model by remember { mutableStateOf("") }
  Column(modifier = Modifier.padding(16.dp)) {
    Text("Model Settings")
    OutlinedTextField(value = baseUrl, onValueChange = { baseUrl = it }, label = { Text("Base URL") })
    OutlinedTextField(value = model, onValueChange = { model = it }, label = { Text("Model") })
  }
}
```

Create `apps/android/app/src/main/java/com/generalagent/mobile/ui/SkillsScreen.kt`:

```kotlin
package com.generalagent.mobile.ui

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp

@Composable
fun SkillsScreen() {
  Column(modifier = Modifier.padding(16.dp)) {
    Text("Skills")
    Text("No skills available")
  }
}
```

Create `apps/android/app/src/main/java/com/generalagent/mobile/ui/DiagnosticsScreen.kt`:

```kotlin
package com.generalagent.mobile.ui

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.generalagent.mobile.runtime.RuntimeDiagnostics

@Composable
fun DiagnosticsScreen(diagnostics: RuntimeDiagnostics) {
  Column(modifier = Modifier.padding(16.dp)) {
    Text("Diagnostics")
    Text("Platform: ${diagnostics.platform}")
    Text("Database: ${if (diagnostics.databaseReady) "ready" else "unavailable"}")
    Text("Capabilities: ${diagnostics.capabilities.joinToString()}")
  }
}
```

- [ ] **Step 5: Wire root from activity**

Modify `apps/android/app/src/main/java/com/generalagent/mobile/MainActivity.kt`:

```kotlin
package com.generalagent.mobile

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.material3.MaterialTheme
import com.generalagent.mobile.runtime.RuntimeBridge
import com.generalagent.mobile.ui.AppRoot

class MainActivity : ComponentActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    val diagnostics = RuntimeBridge(this).load().diagnostics()
    setContent {
      MaterialTheme {
        AppRoot(diagnostics = diagnostics)
      }
    }
  }
}
```

- [ ] **Step 6: Run Android tests and assemble**

Run: `pixi run android-test`

Expected: PASS.

Run: `pixi run android-assemble`

Expected: PASS and creates `apps/android/app/build/outputs/apk/debug/app-debug.apk`.

- [ ] **Step 7: Commit**

```bash
git add apps/android
git commit -m "feat: add android compose mvp screens"
```

### Task 10: End-to-End Verification and Acceptance Harness

**Files:**
- Create: `scripts/mobile-mvp-check.mjs`
- Modify: `pixi.toml`
- Modify: `docs/mvp-verification.md`

**Interfaces:**
- Consumes: all previous tasks
- Produces: one command that verifies Rust, FFI, and Android unit/build checks

- [ ] **Step 1: Add verification script test by running absent command**

Run: `pixi run mobile-mvp-check`

Expected: FAIL because the task does not exist.

- [ ] **Step 2: Add verification script**

Create `scripts/mobile-mvp-check.mjs`:

```javascript
import { spawnSync } from "node:child_process";

const checks = [
  ["cargo", ["test", "-p", "agent-runtime"]],
  ["cargo", ["test", "-p", "mobile-ffi"]],
  ["bash", ["-lc", "cd apps/android && ./gradlew :app:testDebugUnitTest"]],
  ["bash", ["-lc", "cd apps/android && ./gradlew :app:assembleDebug"]],
];

for (const [command, args] of checks) {
  const label = `${command} ${args.join(" ")}`;
  console.log(`\n==> ${label}`);
  const result = spawnSync(command, args, { stdio: "inherit" });
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}
```

Modify `pixi.toml`:

```toml
mobile-mvp-check = "node scripts/mobile-mvp-check.mjs"
```

- [ ] **Step 3: Update verification docs**

Append to `docs/mvp-verification.md`:

```markdown
## Android-First Mobile Runtime MVP

Run the combined mobile MVP check:

```bash
pixi run mobile-mvp-check
```

Expected result:

- `cargo test -p agent-runtime` passes.
- `cargo test -p mobile-ffi` passes.
- `./gradlew :app:testDebugUnitTest` passes in `apps/android`.
- `./gradlew :app:assembleDebug` produces `apps/android/app/build/outputs/apk/debug/app-debug.apk`.

The MVP is accepted only when the Android app initializes the local Rust runtime, reports Android MVP capabilities, stores model secrets outside Rust persistence, and builds a debug APK without depending on desktop or `agent-server`.
```
```

- [ ] **Step 4: Run combined verification**

Run: `pixi run mobile-mvp-check`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add pixi.toml scripts/mobile-mvp-check.mjs docs/mvp-verification.md
git commit -m "test: add mobile mvp verification"
```

## Self-Review Checklist

- Spec coverage:
  - Android-independent runtime: Tasks 1, 4, 5.
  - HTTP model path: Task 5 wires `StoredModelConfig`, `SecretResolver`, and `GatewayHttpClient`.
  - Keystore secrets: Task 8.
  - capability filtering: Tasks 1 and 2.
  - app-data VFS: Task 3.
  - Android MVP UI: Tasks 7 and 9.
  - combined verification: Task 10.
- Vague-marker scan:
  - No task uses vague markers or undefined follow-up instructions.
  - Each task has concrete files, commands, and expected results.
- Type consistency:
  - `CapabilitySet`, `PlatformId`, `SkillCapabilityMetadata`, `AppDataVfs`, `MobileRuntimeHost`, and `MobileRuntime` are introduced before use by later tasks.
  - Android `RuntimeDiagnostics` mirrors the initial FFI diagnostics fields.

## Execution Notes

Use a clean branch or the current main branch according to the repository instruction in AGENTS.md. Do not include unrelated dirty worktree changes in task commits. If Android SDK or emulator tooling is missing, stop after Rust and Android unit-test scaffolding and report the exact missing environment variable or SDK package.
