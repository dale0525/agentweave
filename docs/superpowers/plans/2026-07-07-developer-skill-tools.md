# Developer Skill Tools Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build development-only tools for inspecting, validating, deleting, and Codex-guiding local GeneralAgent skill packages.

**Architecture:** Keep production skill behavior unchanged. Add a backend development inventory layer that scans the configured `skills/` root and exposes it through `/dev/skills` routes, then add a `#developer` desktop screen that consumes those routes and generates `skill-creator` prompts. Runtime skills still come from `skill.json`; instruction skills still come from `SKILL.md`.

**Tech Stack:** Rust 2024, Axum, Tokio, React 18, TypeScript, Vite, Vitest, Radix Dialog, lucide-react, Stitch.

## Global Constraints

- Chat with the user in zh_CN.
- Use English for code and UI copy unless a file is already localized.
- Keep source-like files under 1000 physical lines.
- Use `pixi` for all project commands.
- Current branch is `main`; develop in the main branch and do not create a worktree.
- Do not revert unrelated uncommitted changes already present in the worktree.
- Configure git identity as `Logic Tan <logictan89@gmail.com>` before commits.
- Development routes must remain absent unless `GENERAL_AGENT_DEV_API=1` mounts `router_with_dev_routes`.
- UI implementation must follow these Stitch screens:
  - Project: `8616130577965446903`
  - Desktop: `projects/8616130577965446903/screens/490091d713474aa784d1b42ce510af7b`
  - Mobile: `projects/8616130577965446903/screens/333c3ec8116c4d7799f16c12d2d8dcdb`
- Desktop visual check viewport: `1280x900`.
- Mobile visual check viewport: `390x844`.
- Use Radix Dialog for modal confirmation/prompt surfaces and lucide-react icons for icon buttons.
- Do not expose skill management in the end-user chat flow.

---

## File Structure

- Modify `crates/agent-runtime/src/skill.rs`: expose a single-package runtime skill loader that reuses existing manifest validation.
- Modify `crates/agent-runtime/src/skill_catalog.rs`: expose a single-file instruction skill summary reader that reuses existing `SKILL.md` front matter validation.
- Create `crates/agent-server/src/dev_skills.rs`: scan skill package directories, collect structured package metadata, compute validation results, and delete safe package directories.
- Modify `crates/agent-server/src/main.rs`: register `dev_skills` module and pass the configured skills root into application state.
- Modify `crates/agent-server/src/api.rs`: store `skills_root`, expose a getter, allow `DELETE` in desktop CORS, and keep production routes unchanged.
- Modify `crates/agent-server/src/dev_api.rs`: mount and implement `/dev/skills`, `/dev/skills/validate`, `/dev/skills/reload`, and `DELETE /dev/skills/{id}`.
- Modify `apps/desktop/src/renderer/api.ts`: add dev skill API types and HTTP helpers.
- Create `apps/desktop/src/renderer/devSkillPrompts.ts`: build deterministic Codex `skill-creator` prompts from inventory data.
- Create `apps/desktop/src/renderer/screens/DeveloperTools.tsx`: development workbench screen.
- Create `apps/desktop/src/renderer/components/developer/SkillPackageList.tsx`: compact package list and filters.
- Create `apps/desktop/src/renderer/components/developer/SkillPackageDetail.tsx`: selected package diagnostics and actions.
- Create `apps/desktop/src/renderer/components/developer/SkillCreatorPromptDialog.tsx`: copyable prompt modal.
- Create `apps/desktop/src/renderer/components/developer/DeleteSkillDialog.tsx`: destructive confirmation modal.
- Create `apps/desktop/src/renderer/components/SettingsDeveloperTools.tsx`: dev-only settings entry point that hides itself when the dev API is unavailable.
- Modify `apps/desktop/src/renderer/App.tsx`: add `developer` hash route.
- Modify `apps/desktop/src/renderer/screens/Settings.tsx`: accept and render the dev tools entry point.
- Create `apps/desktop/src/renderer/styles/developer.css`: Stitch-matched workbench styling using existing tokens and component conventions.
- Modify `apps/desktop/src/renderer/styles/index.css`: import `developer.css`.
- Create `apps/desktop/tests/developer-tools.test.tsx`: frontend route, API, prompt, and dialog tests.

---

### Task 1: Runtime Helpers And Skill Inventory Scanner

**Files:**
- Modify: `crates/agent-runtime/src/skill.rs`
- Modify: `crates/agent-runtime/src/skill_catalog.rs`
- Create: `crates/agent-server/src/dev_skills.rs`
- Modify: `crates/agent-server/src/main.rs`

**Interfaces:**
- Consumes: existing `SkillManifest`, `SkillRegistry`, `SkillCatalog`, `SkillSummary`.
- Produces:
  - `SkillRegistry::load_development_skill(root: impl AsRef<Path>) -> anyhow::Result<InstalledSkill>`
  - `SkillCatalog::read_development_skill_summary(root: impl AsRef<Path>, skill_path: impl AsRef<Path>) -> anyhow::Result<SkillSummary>`
  - `dev_skills::scan_skill_packages(root: impl AsRef<Path>) -> anyhow::Result<DevSkillInventory>`
  - `dev_skills::delete_skill_package(root: impl AsRef<Path>, id: &str) -> anyhow::Result<DevSkillInventory>`

- [ ] **Step 1: Write failing runtime helper tests**

Add tests to `crates/agent-runtime/src/skill.rs`:

```rust
#[tokio::test]
async fn load_development_skill_validates_one_runtime_package() {
    let root = unique_test_dir("single-runtime-package");
    write_echo_skill(&root, "echo", "echo").await;

    let skill = SkillRegistry::load_development_skill(root.join("echo"))
        .await
        .unwrap();

    assert_eq!(skill.manifest.name, "echo");
    assert_eq!(skill.manifest.tools[0].name, "echo");
    remove_test_dir(root).await;
}
```

Add tests to `crates/agent-runtime/src/skill_catalog.rs`:

```rust
#[tokio::test]
async fn read_development_skill_summary_validates_one_skill_file() {
    let root = unique_test_dir("single-instruction-package");
    write_skill_md(
        &root,
        "planning",
        "---\nname: planning\ndescription: Plan work.\n---\n\n# Planning",
    )
    .await;

    let summary = SkillCatalog::read_development_skill_summary(
        &root,
        root.join("planning").join("SKILL.md"),
    )
    .await
    .unwrap();

    assert_eq!(summary.name, "planning");
    assert_eq!(summary.source, PathBuf::from("planning/SKILL.md"));
    remove_test_dir(root).await;
}
```

- [ ] **Step 2: Run helper tests to verify they fail**

Run:

```bash
pixi run cargo test -p agent-runtime development_skill -- --nocapture
```

Expected: FAIL because `load_development_skill` and `read_development_skill_summary` do not exist.

- [ ] **Step 3: Implement runtime helper exports**

In `crates/agent-runtime/src/skill.rs`, add this public method inside `impl SkillRegistry`:

```rust
pub async fn load_development_skill(root: impl AsRef<Path>) -> anyhow::Result<InstalledSkill> {
    Self::load_skill(root.as_ref().to_path_buf()).await
}
```

In `crates/agent-runtime/src/skill_catalog.rs`, add this public method inside `impl SkillCatalog`:

```rust
pub async fn read_development_skill_summary(
    root: impl AsRef<Path>,
    skill_path: impl AsRef<Path>,
) -> anyhow::Result<SkillSummary> {
    let root = root.as_ref();
    let canonical_root = tokio::fs::canonicalize(root)
        .await
        .with_context(|| format!("failed to resolve skill root {}", root.display()))?;
    read_skill_summary(&canonical_root, skill_path.as_ref()).await
}
```

- [ ] **Step 4: Run helper tests to verify they pass**

Run:

```bash
pixi run cargo test -p agent-runtime development_skill -- --nocapture
```

Expected: PASS for both tests.

- [ ] **Step 5: Write failing inventory scanner tests**

Create `crates/agent-server/src/dev_skills.rs` with tests first. Include these public types at the top of the file so tests compile after the implementation step:

```rust
use agent_runtime::{
    skill::{SkillManifest, SkillRegistry},
    skill_catalog::SkillCatalog,
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap},
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DevSkillInventory {
    pub root: String,
    pub packages: Vec<DevSkillPackage>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DevSkillPackage {
    pub id: String,
    pub path: String,
    pub name: String,
    pub description: String,
    pub has_skill_md: bool,
    pub has_runtime_manifest: bool,
    pub runtime_tools: Vec<String>,
    pub package_kind: DevSkillPackageKind,
    pub bundle_ready: bool,
    pub validation: DevSkillValidation,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DevSkillPackageKind {
    Runtime,
    Instruction,
    Combined,
    Empty,
    Invalid,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DevSkillValidation {
    pub ok: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}
```

Add tests in the same file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::fs;

    #[tokio::test]
    async fn scan_reports_package_kinds_and_partial_errors() {
        let root = unique_test_dir("scan-kinds");
        write_runtime_skill(&root, "runtime-only", "runtime-only", "runtime_echo").await;
        write_instruction_skill(&root, "instruction-only", "planning", "Plan work.").await;
        write_runtime_skill(&root, "combined", "combined", "combined_echo").await;
        write_instruction_skill(&root, "combined", "combined", "Combined instructions.").await;
        fs::create_dir_all(root.join("empty")).await.unwrap();
        fs::create_dir_all(root.join("invalid")).await.unwrap();
        fs::write(root.join("invalid").join("skill.json"), "{not json")
            .await
            .unwrap();

        let inventory = scan_skill_packages(&root).await.unwrap();
        let packages = packages_by_id(&inventory);

        assert_eq!(packages["runtime-only"].package_kind, DevSkillPackageKind::Runtime);
        assert_eq!(
            packages["instruction-only"].package_kind,
            DevSkillPackageKind::Instruction
        );
        assert_eq!(packages["combined"].package_kind, DevSkillPackageKind::Combined);
        assert_eq!(packages["empty"].package_kind, DevSkillPackageKind::Empty);
        assert_eq!(packages["invalid"].package_kind, DevSkillPackageKind::Invalid);
        assert!(packages["runtime-only"].bundle_ready);
        assert!(!packages["instruction-only"].bundle_ready);
        assert!(!packages["invalid"].validation.ok);
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn scan_reports_duplicate_runtime_tools_and_instruction_names() {
        let root = unique_test_dir("scan-duplicates");
        write_runtime_skill(&root, "runtime-a", "runtime-a", "shared_tool").await;
        write_runtime_skill(&root, "runtime-b", "runtime-b", "shared_tool").await;
        write_instruction_skill(&root, "instruction-a", "shared", "First.").await;
        write_instruction_skill(&root, "instruction-b", "shared", "Second.").await;

        let inventory = scan_skill_packages(&root).await.unwrap();
        let packages = packages_by_id(&inventory);

        assert!(
            packages["runtime-a"]
                .validation
                .errors
                .iter()
                .any(|error| error.contains("duplicate runtime tool name: shared_tool"))
        );
        assert!(
            packages["instruction-b"]
                .validation
                .errors
                .iter()
                .any(|error| error.contains("duplicate instruction skill name: shared"))
        );
        remove_test_dir(root).await;
    }

    fn packages_by_id(inventory: &DevSkillInventory) -> BTreeMap<String, DevSkillPackage> {
        inventory
            .packages
            .iter()
            .cloned()
            .map(|package| (package.id.clone(), package))
            .collect()
    }
}
```

- [ ] **Step 6: Run scanner tests to verify they fail**

Run:

```bash
pixi run cargo test -p agent-server dev_skills -- --nocapture
```

Expected: FAIL because `scan_skill_packages` and helpers are not implemented.

- [ ] **Step 7: Implement scanner and duplicate checks**

In `crates/agent-server/src/main.rs`, add:

```rust
mod dev_skills;
```

In `crates/agent-server/src/dev_skills.rs`, implement these functions:

```rust
pub async fn scan_skill_packages(root: impl AsRef<Path>) -> anyhow::Result<DevSkillInventory> {
    let root = root.as_ref();
    let canonical_root = ensure_skills_root(root).await?;
    let mut packages = Vec::new();
    let mut entries = tokio::fs::read_dir(&canonical_root).await?;

    while let Some(entry) = entries.next_entry().await? {
        let file_type = entry.file_type().await?;
        if !file_type.is_dir() {
            continue;
        }
        packages.push(scan_one_package(&canonical_root, entry.path()).await);
    }

    packages.sort_by(|left, right| left.id.cmp(&right.id));
    apply_duplicate_diagnostics(&mut packages);

    Ok(DevSkillInventory {
        root: canonical_root.display().to_string(),
        packages,
    })
}

pub async fn delete_skill_package(
    root: impl AsRef<Path>,
    id: &str,
) -> anyhow::Result<DevSkillInventory> {
    let root = root.as_ref();
    let canonical_root = ensure_skills_root(root).await?;
    validate_package_id(id)?;
    let target = canonical_root.join(id);
    let canonical_target = tokio::fs::canonicalize(&target).await?;
    if !canonical_target.starts_with(&canonical_root) {
        anyhow::bail!("unsafe skill package path: {id}");
    }
    if !tokio::fs::metadata(&canonical_target).await?.is_dir() {
        anyhow::bail!("skill package is not a directory: {id}");
    }
    tokio::fs::remove_dir_all(&canonical_target).await?;
    scan_skill_packages(&canonical_root).await
}
```

Use these exact helper names so downstream tasks can call them:

```rust
async fn ensure_skills_root(root: &Path) -> anyhow::Result<PathBuf>;
async fn scan_one_package(root: &Path, package_path: PathBuf) -> DevSkillPackage;
fn apply_duplicate_diagnostics(packages: &mut [DevSkillPackage]);
fn validate_package_id(id: &str) -> anyhow::Result<()>;
fn compute_package_kind(has_skill_md: bool, has_runtime_manifest: bool, ok: bool) -> DevSkillPackageKind;
```

Rules for the implementation:

- If `skill.json` exists, call `SkillRegistry::load_development_skill(&package_path).await`.
- If `SKILL.md` exists, call `SkillCatalog::read_development_skill_summary(root, package_path.join("SKILL.md")).await`.
- On parse or validation failure, push the error text into `validation.errors`.
- Set `bundle_ready` to `has_runtime_manifest && validation.ok && !runtime_tools.is_empty()`.
- Set `name` from `skill.json` when valid; otherwise from `SKILL.md`; otherwise from the directory id.
- Set `description` from `skill.json` when valid; otherwise from `SKILL.md`; otherwise to `"No skill metadata found."`.
- `validate_package_id` accepts only one `Component::Normal(_)` path segment and rejects empty ids, absolute paths, separators, and `..`.

- [ ] **Step 8: Run scanner tests to verify they pass**

Run:

```bash
pixi run cargo test -p agent-server dev_skills -- --nocapture
```

Expected: PASS.

- [ ] **Step 9: Commit Task 1**

Run:

```bash
git add crates/agent-runtime/src/skill.rs crates/agent-runtime/src/skill_catalog.rs crates/agent-server/src/dev_skills.rs crates/agent-server/src/main.rs
git diff --cached --name-only
git commit -m "feat: add development skill inventory scanner"
```

Expected staged files:

```text
crates/agent-runtime/src/skill.rs
crates/agent-runtime/src/skill_catalog.rs
crates/agent-server/src/dev_skills.rs
crates/agent-server/src/main.rs
```

---

### Task 2: Development Skill HTTP API

**Files:**
- Modify: `crates/agent-server/src/api.rs`
- Modify: `crates/agent-server/src/dev_api.rs`
- Modify: `crates/agent-server/src/main.rs`
- Test: `crates/agent-server/src/dev_api.rs`

**Interfaces:**
- Consumes: `dev_skills::scan_skill_packages`, `dev_skills::delete_skill_package`, `AppState::skills_root`.
- Produces:
  - `GET /dev/skills`
  - `POST /dev/skills/validate`
  - `POST /dev/skills/reload`
  - `DELETE /dev/skills/{id}`
  - `AppState::with_skills_root(skills_root: PathBuf) -> Self`
  - `AppState::skills_root(&self) -> Option<PathBuf>`

- [ ] **Step 1: Write failing API tests**

In `crates/agent-server/src/dev_api.rs`, add tests:

```rust
#[tokio::test]
async fn dev_skills_route_is_not_mounted_by_default() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let app = crate::api::router(Arc::new(crate::api::AppState::new(storage)));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dev/skills")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn dev_skills_route_returns_inventory_when_enabled() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let skills_root = development_skills().await;
    let skills = SkillRegistry::load_development(&skills_root).await.unwrap();
    let state = Arc::new(
        crate::api::AppState::new_with_agent_and_skills(storage, Arc::new(TestAgent), skills)
            .with_skills_root(skills_root.clone()),
    );
    let app = crate::api::router_with_dev_routes(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dev/skills")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = read_json(response).await;
    assert_eq!(body["packages"][0]["id"], "echo");
    assert_eq!(body["packages"][0]["packageKind"], "runtime");
    remove_test_dir(skills_root).await;
}

#[tokio::test]
async fn dev_delete_skill_rejects_unsafe_id() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let skills_root = development_skills().await;
    let skills = SkillRegistry::load_development(&skills_root).await.unwrap();
    let state = Arc::new(
        crate::api::AppState::new_with_agent_and_skills(storage, Arc::new(TestAgent), skills)
            .with_skills_root(skills_root.clone()),
    );
    let app = crate::api::router_with_dev_routes(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/dev/skills/..%2Fecho")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(skills_root.join("echo").exists());
    remove_test_dir(skills_root).await;
}

#[tokio::test]
async fn dev_delete_skill_removes_package_and_returns_inventory() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let skills_root = development_skills().await;
    let skills = SkillRegistry::load_development(&skills_root).await.unwrap();
    let state = Arc::new(
        crate::api::AppState::new_with_agent_and_skills(storage, Arc::new(TestAgent), skills)
            .with_skills_root(skills_root.clone()),
    );
    let app = crate::api::router_with_dev_routes(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/dev/skills/echo")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(!skills_root.join("echo").exists());
    let body = read_json(response).await;
    assert_eq!(body["packages"].as_array().unwrap().len(), 0);
    remove_test_dir(skills_root).await;
}
```

- [ ] **Step 2: Run API tests to verify they fail**

Run:

```bash
pixi run cargo test -p agent-server dev_ -- --nocapture
```

Expected: FAIL because `/dev/skills` routes and `with_skills_root` do not exist.

- [ ] **Step 3: Add skills root to app state**

In `crates/agent-server/src/api.rs`, add field:

```rust
skills_root: Option<PathBuf>,
```

Initialize it as `None` in test constructors and `Some` through a builder:

```rust
pub fn with_skills_root(mut self, skills_root: PathBuf) -> Self {
    self.skills_root = Some(skills_root);
    self
}

pub(crate) fn skills_root(&self) -> Option<PathBuf> {
    self.skills_root.clone()
}
```

In `crates/agent-server/src/main.rs`, set it when building state:

```rust
let state = Arc::new(
    api::AppState::new_with_agent_and_skills(storage, Arc::new(runner), skills)
        .with_runtime_config(runtime_config)
        .with_skill_catalog(skill_catalog)
        .with_skills_root(skills_root.clone()),
);
```

- [ ] **Step 4: Add dev skill routes**

In `crates/agent-server/src/dev_api.rs`, import `delete` and `Path`:

```rust
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
};
```

Add routes:

```rust
.route("/dev/skills", get(list_skills))
.route("/dev/skills/validate", post(validate_skills))
.route("/dev/skills/reload", post(reload_skills))
.route("/dev/skills/{skill_id}", delete(delete_skill))
```

Add handlers:

```rust
async fn list_skills(
    State(state): State<Arc<AppState>>,
) -> Result<Json<crate::dev_skills::DevSkillInventory>, StatusCode> {
    let root = state.skills_root().ok_or(StatusCode::NOT_FOUND)?;
    crate::dev_skills::scan_skill_packages(root)
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn validate_skills(
    State(state): State<Arc<AppState>>,
) -> Result<Json<crate::dev_skills::DevSkillInventory>, StatusCode> {
    list_skills(State(state)).await
}

async fn reload_skills(
    State(state): State<Arc<AppState>>,
) -> Result<Json<crate::dev_skills::DevSkillInventory>, StatusCode> {
    list_skills(State(state)).await
}

async fn delete_skill(
    Path(skill_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<crate::dev_skills::DevSkillInventory>, StatusCode> {
    let root = state.skills_root().ok_or(StatusCode::NOT_FOUND)?;
    crate::dev_skills::delete_skill_package(root, &skill_id)
        .await
        .map(Json)
        .map_err(|error| {
            let message = error.to_string();
            if message.contains("unsafe") || message.contains("invalid") {
                StatusCode::BAD_REQUEST
            } else if message.contains("No such file") || message.contains("not found") {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        })
}
```

- [ ] **Step 5: Allow DELETE in desktop CORS**

In `crates/agent-server/src/api.rs`, update:

```rust
.allow_methods([Method::GET, Method::POST, Method::DELETE])
```

- [ ] **Step 6: Run API tests to verify they pass**

Run:

```bash
pixi run cargo test -p agent-server dev_ -- --nocapture
```

Expected: PASS.

- [ ] **Step 7: Commit Task 2**

Run:

```bash
git add crates/agent-server/src/api.rs crates/agent-server/src/dev_api.rs crates/agent-server/src/main.rs
git diff --cached --name-only
git commit -m "feat: expose development skill routes"
```

Expected staged files:

```text
crates/agent-server/src/api.rs
crates/agent-server/src/dev_api.rs
crates/agent-server/src/main.rs
```

---

### Task 3: Frontend Dev Skill API And Prompt Builder

**Files:**
- Modify: `apps/desktop/src/renderer/api.ts`
- Create: `apps/desktop/src/renderer/devSkillPrompts.ts`
- Create: `apps/desktop/tests/developer-tools.test.tsx`

**Interfaces:**
- Consumes: `/dev/skills` inventory JSON from Task 2.
- Produces:
  - `DevSkillInventory`, `DevSkillPackage`, `DevSkillPackageKind`, `DevSkillValidation`
  - `listDevSkills(): Promise<DevSkillInventory>`
  - `validateDevSkills(): Promise<DevSkillInventory>`
  - `reloadDevSkills(): Promise<DevSkillInventory>`
  - `deleteDevSkill(id: string): Promise<DevSkillInventory>`
  - `buildCreateSkillPrompt(root: string): string`
  - `buildModifySkillPrompt(root: string, skillPackage: DevSkillPackage): string`

- [ ] **Step 1: Write failing prompt tests**

In `apps/desktop/tests/developer-tools.test.tsx`, add:

```tsx
import { describe, expect, it } from "vitest";

import {
  buildCreateSkillPrompt,
  buildModifySkillPrompt
} from "../src/renderer/devSkillPrompts";
import { DevSkillPackage } from "../src/renderer/api";

describe("developer skill prompts", () => {
  it("builds a create prompt for Codex skill-creator", () => {
    const prompt = buildCreateSkillPrompt("/repo/skills");

    expect(prompt).toContain("Use the existing skill-creator skill");
    expect(prompt).toContain("/repo/skills");
    expect(prompt).toContain("SKILL.md is a development authoring asset");
    expect(prompt).toContain("skill.json is the GeneralAgent runtime contract");
  });

  it("builds a modify prompt with package diagnostics", () => {
    const skillPackage: DevSkillPackage = {
      id: "echo",
      path: "echo",
      name: "echo",
      description: "Echo a text payload.",
      hasSkillMd: false,
      hasRuntimeManifest: true,
      runtimeTools: ["echo"],
      packageKind: "runtime",
      bundleReady: true,
      validation: {
        ok: false,
        errors: ["missing SKILL.md is informational only"],
        warnings: []
      }
    };

    const prompt = buildModifySkillPrompt("/repo/skills", skillPackage);

    expect(prompt).toContain("Use the existing skill-creator skill");
    expect(prompt).toContain("/repo/skills/echo");
    expect(prompt).toContain("runtime tools: echo");
    expect(prompt).toContain("missing SKILL.md is informational only");
  });
});
```

- [ ] **Step 2: Run prompt tests to verify they fail**

Run:

```bash
cd apps/desktop && pixi run npm test -- developer-tools.test.tsx
```

Expected: FAIL because `devSkillPrompts.ts` and API types do not exist.

- [ ] **Step 3: Add frontend API types and helpers**

In `apps/desktop/src/renderer/api.ts`, add:

```ts
export type DevSkillPackageKind =
  | "runtime"
  | "instruction"
  | "combined"
  | "empty"
  | "invalid";

export type DevSkillValidation = {
  ok: boolean;
  errors: string[];
  warnings: string[];
};

export type DevSkillPackage = {
  id: string;
  path: string;
  name: string;
  description: string;
  hasSkillMd: boolean;
  hasRuntimeManifest: boolean;
  runtimeTools: string[];
  packageKind: DevSkillPackageKind;
  bundleReady: boolean;
  validation: DevSkillValidation;
};

export type DevSkillInventory = {
  root: string;
  packages: DevSkillPackage[];
};

export async function listDevSkills(): Promise<DevSkillInventory> {
  return requestJson<DevSkillInventory>("/dev/skills", { method: "GET" });
}

export async function validateDevSkills(): Promise<DevSkillInventory> {
  return requestJson<DevSkillInventory>("/dev/skills/validate", {
    method: "POST"
  });
}

export async function reloadDevSkills(): Promise<DevSkillInventory> {
  return requestJson<DevSkillInventory>("/dev/skills/reload", {
    method: "POST"
  });
}

export async function deleteDevSkill(id: string): Promise<DevSkillInventory> {
  return requestJson<DevSkillInventory>(`/dev/skills/${encodeURIComponent(id)}`, {
    method: "DELETE"
  });
}
```

- [ ] **Step 4: Add deterministic prompt builder**

Create `apps/desktop/src/renderer/devSkillPrompts.ts`:

```ts
import { DevSkillPackage } from "./api";

export function buildCreateSkillPrompt(root: string): string {
  return [
    "Use the existing skill-creator skill to create a new GeneralAgent skill package.",
    "",
    `Target skills root: ${root}`,
    "",
    "Requirements:",
    "- Create the package under the target skills root.",
    "- SKILL.md is a development authoring asset for Codex guidance.",
    "- skill.json is the GeneralAgent runtime contract for packaged tools.",
    "- Add or update skill.json only when the package needs runtime tools.",
    "- Keep generated source files focused and under 1000 physical lines.",
    "",
    "After creating the package, run the GeneralAgent development skill validation."
  ].join("\n");
}

export function buildModifySkillPrompt(
  root: string,
  skillPackage: DevSkillPackage
): string {
  const runtimeTools =
    skillPackage.runtimeTools.length > 0
      ? skillPackage.runtimeTools.join(", ")
      : "none";
  const errors =
    skillPackage.validation.errors.length > 0
      ? skillPackage.validation.errors.map((error) => `- ${error}`).join("\n")
      : "- none";
  const warnings =
    skillPackage.validation.warnings.length > 0
      ? skillPackage.validation.warnings.map((warning) => `- ${warning}`).join("\n")
      : "- none";

  return [
    "Use the existing skill-creator skill to modify this GeneralAgent skill package.",
    "",
    `Package path: ${root}/${skillPackage.path}`,
    `Package name: ${skillPackage.name}`,
    `Description: ${skillPackage.description}`,
    `Package kind: ${skillPackage.packageKind}`,
    `Files present: SKILL.md=${skillPackage.hasSkillMd}, skill.json=${skillPackage.hasRuntimeManifest}`,
    `runtime tools: ${runtimeTools}`,
    `Bundle ready: ${skillPackage.bundleReady}`,
    "",
    "Validation errors:",
    errors,
    "",
    "Validation warnings:",
    warnings,
    "",
    "Remember: SKILL.md is a development authoring asset; skill.json is the GeneralAgent runtime contract."
  ].join("\n");
}
```

- [ ] **Step 5: Run prompt tests to verify they pass**

Run:

```bash
cd apps/desktop && pixi run npm test -- developer-tools.test.tsx
```

Expected: PASS for the prompt tests.

- [ ] **Step 6: Commit Task 3**

Run:

```bash
git add apps/desktop/src/renderer/api.ts apps/desktop/src/renderer/devSkillPrompts.ts apps/desktop/tests/developer-tools.test.tsx
git diff --cached --name-only
git commit -m "feat: add developer skill frontend API"
```

Expected staged files:

```text
apps/desktop/src/renderer/api.ts
apps/desktop/src/renderer/devSkillPrompts.ts
apps/desktop/tests/developer-tools.test.tsx
```

---

### Task 4: Developer Tools Screen

**Files:**
- Create: `apps/desktop/src/renderer/screens/DeveloperTools.tsx`
- Create: `apps/desktop/src/renderer/components/developer/SkillPackageList.tsx`
- Create: `apps/desktop/src/renderer/components/developer/SkillPackageDetail.tsx`
- Create: `apps/desktop/src/renderer/components/developer/SkillCreatorPromptDialog.tsx`
- Create: `apps/desktop/src/renderer/components/developer/DeleteSkillDialog.tsx`
- Create: `apps/desktop/src/renderer/styles/developer.css`
- Modify: `apps/desktop/src/renderer/styles/index.css`
- Test: `apps/desktop/tests/developer-tools.test.tsx`

**Interfaces:**
- Consumes: API helpers and prompt builders from Task 3.
- Produces:
  - `DeveloperTools({ onBack }: { onBack: () => void }): JSX.Element`
  - `SkillPackageList` selected package callback
  - `SkillPackageDetail` action callbacks
  - Radix dialogs for prompt and delete flows

- [ ] **Step 1: Write failing screen tests**

Update the import block at the top of `apps/desktop/tests/developer-tools.test.tsx` to this combined block, then add the screen tests below the prompt tests:

```tsx
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { DevSkillPackage } from "../src/renderer/api";
import { DeveloperTools } from "../src/renderer/screens/DeveloperTools";
import {
  buildCreateSkillPrompt,
  buildModifySkillPrompt
} from "../src/renderer/devSkillPrompts";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

it("renders package inventory and selected runtime-only details", async () => {
  mockFetch([
    jsonResponse({
      root: "/repo/skills",
      packages: [
        {
          id: "echo",
          path: "echo",
          name: "echo",
          description: "Echo a text payload.",
          hasSkillMd: false,
          hasRuntimeManifest: true,
          runtimeTools: ["echo"],
          packageKind: "runtime",
          bundleReady: true,
          validation: { ok: true, errors: [], warnings: [] }
        }
      ]
    })
  ]);

  render(<DeveloperTools onBack={() => undefined} />);

  expect(await screen.findByText("Skill packages")).toBeInTheDocument();
  expect(screen.getByText("echo")).toBeInTheDocument();
  expect(screen.getByText("skills/echo")).toBeInTheDocument();
  expect(screen.getByText("SKILL.md missing")).toBeInTheDocument();
  expect(screen.queryByText("Broken")).not.toBeInTheDocument();
});

it("shows a disabled state when the development API is unavailable", async () => {
  mockFetch([new Response(JSON.stringify({ error: "not found" }), { status: 404 })]);

  render(<DeveloperTools onBack={() => undefined} />);

  expect(
    await screen.findByText("Development API is not available")
  ).toBeInTheDocument();
});

it("opens a skill-creator prompt dialog for a selected package", async () => {
  const user = userEvent.setup();
  mockFetch([
    jsonResponse({
      root: "/repo/skills",
      packages: [
        {
          id: "echo",
          path: "echo",
          name: "echo",
          description: "Echo a text payload.",
          hasSkillMd: false,
          hasRuntimeManifest: true,
          runtimeTools: ["echo"],
          packageKind: "runtime",
          bundleReady: true,
          validation: { ok: true, errors: [], warnings: [] }
        }
      ]
    })
  ]);

  render(<DeveloperTools onBack={() => undefined} />);

  await user.click(await screen.findByRole("button", { name: "Modify with skill-creator" }));

  expect(screen.getByRole("dialog", { name: "skill-creator prompt" })).toBeInTheDocument();
  expect(screen.getByText(/Use the existing skill-creator skill/)).toBeInTheDocument();
});

it("deletes a package after confirmation and refreshes inventory", async () => {
  const user = userEvent.setup();
  const fetchMock = mockFetch([
    jsonResponse({
      root: "/repo/skills",
      packages: [
        {
          id: "echo",
          path: "echo",
          name: "echo",
          description: "Echo a text payload.",
          hasSkillMd: false,
          hasRuntimeManifest: true,
          runtimeTools: ["echo"],
          packageKind: "runtime",
          bundleReady: true,
          validation: { ok: true, errors: [], warnings: [] }
        }
      ]
    }),
    jsonResponse({ root: "/repo/skills", packages: [] })
  ]);

  render(<DeveloperTools onBack={() => undefined} />);

  await user.click(await screen.findByRole("button", { name: "Delete package" }));
  await user.click(screen.getByRole("button", { name: "Delete echo" }));

  await waitFor(() => expect(screen.getByText("No skill packages found")).toBeInTheDocument());
  expect(fetchMock).toHaveBeenLastCalledWith(
    "http://127.0.0.1:49321/dev/skills/echo",
    expect.objectContaining({ method: "DELETE" })
  );
});
```

Use the existing `mockFetch` and `jsonResponse` helpers from `chat.test.tsx`; if they are local to that file, copy the small helper definitions into `developer-tools.test.tsx`.

- [ ] **Step 2: Run screen tests to verify they fail**

Run:

```bash
cd apps/desktop && pixi run npm test -- developer-tools.test.tsx
```

Expected: FAIL because screen and component files do not exist.

- [ ] **Step 3: Implement `DeveloperTools` screen**

Create `apps/desktop/src/renderer/screens/DeveloperTools.tsx` with this structure:

```tsx
import { ArrowLeft, RefreshCw, ShieldCheck } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import {
  deleteDevSkill,
  DevSkillInventory,
  DevSkillPackage,
  listDevSkills,
  reloadDevSkills,
  validateDevSkills
} from "../api";
import { AppIconButton } from "../components/AppIconButton";
import { DeleteSkillDialog } from "../components/developer/DeleteSkillDialog";
import { SkillCreatorPromptDialog } from "../components/developer/SkillCreatorPromptDialog";
import { SkillPackageDetail } from "../components/developer/SkillPackageDetail";
import { SkillPackageList } from "../components/developer/SkillPackageList";

type DeveloperToolsProps = {
  onBack: () => void;
};

export function DeveloperTools({ onBack }: DeveloperToolsProps): JSX.Element {
  const [inventory, setInventory] = useState<DevSkillInventory | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [promptPackage, setPromptPackage] = useState<DevSkillPackage | "new" | null>(null);
  const [deletePackage, setDeletePackage] = useState<DevSkillPackage | null>(null);

  const selectedPackage = useMemo(
    () => inventory?.packages.find((item) => item.id === selectedId) ?? inventory?.packages[0],
    [inventory, selectedId]
  );

  const loadInventory = async (loader = listDevSkills) => {
    setIsLoading(true);
    setError(null);
    try {
      const nextInventory = await loader();
      setInventory(nextInventory);
      setSelectedId((current) => current ?? nextInventory.packages[0]?.id ?? null);
    } catch {
      setInventory(null);
      setError("Development API is not available");
    } finally {
      setIsLoading(false);
    }
  };

  useEffect(() => {
    void loadInventory();
  }, []);

  const handleDelete = async (skillPackage: DevSkillPackage) => {
    const nextInventory = await deleteDevSkill(skillPackage.id);
    setInventory(nextInventory);
    setSelectedId(nextInventory.packages[0]?.id ?? null);
    setDeletePackage(null);
  };

  return (
    <main className="developer-screen" aria-label="Developer Tools">
      <header className="top-bar developer-top-bar">
        <AppIconButton label="Back to settings" onClick={onBack}>
          <ArrowLeft size={18} aria-hidden="true" />
        </AppIconButton>
        <div className="top-bar-title">
          <h1>Developer Tools</h1>
          <p>{error ? "Development API disconnected" : "Development API connected"}</p>
        </div>
        <AppIconButton label="Refresh skill packages" onClick={() => loadInventory()}>
          <RefreshCw size={18} aria-hidden="true" />
        </AppIconButton>
        <AppIconButton label="Validate all skill packages" onClick={() => loadInventory(validateDevSkills)}>
          <ShieldCheck size={18} aria-hidden="true" />
        </AppIconButton>
      </header>

      <section className="developer-workbench" aria-busy={isLoading}>
        {error ? (
          <div className="developer-empty-state">
            <h2>Development API is not available</h2>
            <p>Start the server with GENERAL_AGENT_DEV_API=1 to manage local skills.</p>
          </div>
        ) : (
          <>
            <SkillPackageList
              inventory={inventory}
              selectedId={selectedPackage?.id ?? null}
              onCreate={() => setPromptPackage("new")}
              onSelect={setSelectedId}
            />
            <SkillPackageDetail
              inventory={inventory}
              skillPackage={selectedPackage ?? null}
              onDelete={setDeletePackage}
              onModify={setPromptPackage}
              onReload={() => loadInventory(reloadDevSkills)}
            />
          </>
        )}
      </section>

      <SkillCreatorPromptDialog
        inventory={inventory}
        promptPackage={promptPackage}
        onOpenChange={(open) => {
          if (!open) setPromptPackage(null);
        }}
      />
      <DeleteSkillDialog
        skillPackage={deletePackage}
        onConfirm={handleDelete}
        onOpenChange={(open) => {
          if (!open) setDeletePackage(null);
        }}
      />
    </main>
  );
}
```

- [ ] **Step 4: Implement developer components**

Create the component files with these exported signatures:

```tsx
export function SkillPackageList(props: {
  inventory: DevSkillInventory | null;
  selectedId: string | null;
  onCreate: () => void;
  onSelect: (id: string) => void;
}): JSX.Element
```

```tsx
export function SkillPackageDetail(props: {
  inventory: DevSkillInventory | null;
  skillPackage: DevSkillPackage | null;
  onDelete: (skillPackage: DevSkillPackage) => void;
  onModify: (skillPackage: DevSkillPackage) => void;
  onReload: () => void;
}): JSX.Element
```

```tsx
export function SkillCreatorPromptDialog(props: {
  inventory: DevSkillInventory | null;
  promptPackage: DevSkillPackage | "new" | null;
  onOpenChange: (open: boolean) => void;
}): JSX.Element
```

```tsx
export function DeleteSkillDialog(props: {
  skillPackage: DevSkillPackage | null;
  onConfirm: (skillPackage: DevSkillPackage) => Promise<void>;
  onOpenChange: (open: boolean) => void;
}): JSX.Element
```

Implementation requirements:

- List rows use `<button type="button">` with stable row height.
- Runtime-only package missing `SKILL.md` renders as text `SKILL.md missing` and does not render `Broken`.
- Package kind labels use title case: `Runtime`, `Instruction`, `Combined`, `Empty`, `Invalid`.
- Prompt dialog uses `Dialog.Root`, `Dialog.Portal`, `Dialog.Overlay`, and `Dialog.Content` from `@radix-ui/react-dialog`.
- Delete dialog confirmation button text is `Delete ${skillPackage.name}`.
- Empty inventory renders `No skill packages found`.

- [ ] **Step 5: Add Stitch-matched styling**

Create `apps/desktop/src/renderer/styles/developer.css` and import it from `styles/index.css`.

Style requirements from Stitch:

- `.developer-workbench` is two columns at desktop: `320px minmax(0, 1fr)`.
- At `max-width: 760px`, `.developer-workbench` becomes one column.
- Use existing color variables from `chat.css`.
- Use 1px borders, white or muted surfaces, 4-8px radii.
- Primary action buttons use teal fill.
- Danger action uses red outline and no fill.
- Prompt preview uses `var(--font-mono)`.
- No gradients, decorative blobs, or card nesting.

- [ ] **Step 6: Run screen tests to verify they pass**

Run:

```bash
cd apps/desktop && pixi run npm test -- developer-tools.test.tsx
```

Expected: PASS.

- [ ] **Step 7: Commit Task 4**

Run:

```bash
git add apps/desktop/src/renderer/screens/DeveloperTools.tsx apps/desktop/src/renderer/components/developer apps/desktop/src/renderer/styles/developer.css apps/desktop/src/renderer/styles/index.css apps/desktop/tests/developer-tools.test.tsx
git diff --cached --name-only
git commit -m "feat: add developer skill tools screen"
```

Expected staged files include the new screen, developer components, `developer.css`, `index.css`, and `developer-tools.test.tsx`.

---

### Task 5: Route And Settings Entry Point

**Files:**
- Modify: `apps/desktop/src/renderer/App.tsx`
- Modify: `apps/desktop/src/renderer/screens/Settings.tsx`
- Create: `apps/desktop/src/renderer/components/SettingsDeveloperTools.tsx`
- Test: `apps/desktop/tests/developer-tools.test.tsx`

**Interfaces:**
- Consumes: `DeveloperTools` from Task 4 and `listDevSkills` from Task 3.
- Produces:
  - App route hash `#developer`
  - `SettingsDeveloperTools({ onOpenDeveloperTools }: { onOpenDeveloperTools: () => void })`

- [ ] **Step 1: Write failing route and entry tests**

Update the same import block in `apps/desktop/tests/developer-tools.test.tsx` to include `App`, then add the route tests below the existing tests:

```tsx
import App from "../src/renderer/App";

it("routes #developer to the developer tools screen", async () => {
  window.history.replaceState(null, "", "/#developer");
  mockFetch([
    jsonResponse({
      root: "/repo/skills",
      packages: []
    })
  ]);

  render(<App />);

  expect(await screen.findByRole("main", { name: "Developer Tools" })).toBeInTheDocument();
});

it("shows settings developer entry only when the dev API is available", async () => {
  const user = userEvent.setup();
  mockFetch([
    jsonResponse({ root: "/repo/skills", packages: [] }),
    jsonResponse({ root: "/repo/skills", packages: [] })
  ]);

  render(<App />);

  await user.click(screen.getByRole("button", { name: "Open settings" }));
  expect(await screen.findByRole("button", { name: "Open developer tools" })).toBeInTheDocument();
});

it("hides settings developer entry when the dev API is unavailable", async () => {
  mockFetch([new Response(JSON.stringify({ error: "not found" }), { status: 404 })]);

  render(<App />);

  await userEvent.click(screen.getByRole("button", { name: "Open settings" }));

  await waitFor(() =>
    expect(screen.queryByRole("button", { name: "Open developer tools" })).not.toBeInTheDocument()
  );
});
```

- [ ] **Step 2: Run route tests to verify they fail**

Run:

```bash
cd apps/desktop && pixi run npm test -- developer-tools.test.tsx
```

Expected: FAIL because route and settings entry do not exist.

- [ ] **Step 3: Add `developer` route**

In `apps/desktop/src/renderer/App.tsx`:

```tsx
import { DeveloperTools } from "./screens/DeveloperTools";

type AppView = "chat" | "settings" | "developer";
```

Update `getViewFromHash`:

```tsx
if (window.location.hash === "#developer") {
  return "developer";
}
if (window.location.hash === "#settings") {
  return "settings";
}
```

Update render:

```tsx
{view === "developer" ? (
  <DeveloperTools onBack={() => navigate("settings")} />
) : view === "settings" ? (
  <Settings
    onBack={() => navigate("chat")}
    onOpenDeveloperTools={() => navigate("developer")}
  />
) : (
  <Chat onOpenSettings={() => navigate("settings")} />
)}
```

- [ ] **Step 4: Add settings developer entry**

Create `apps/desktop/src/renderer/components/SettingsDeveloperTools.tsx`:

```tsx
import { Wrench } from "lucide-react";
import { useEffect, useState } from "react";

import { listDevSkills } from "../api";

type SettingsDeveloperToolsProps = {
  onOpenDeveloperTools: () => void;
};

export function SettingsDeveloperTools({
  onOpenDeveloperTools
}: SettingsDeveloperToolsProps): JSX.Element | null {
  const [isAvailable, setIsAvailable] = useState(false);

  useEffect(() => {
    let active = true;
    listDevSkills()
      .then(() => {
        if (active) setIsAvailable(true);
      })
      .catch(() => {
        if (active) setIsAvailable(false);
      });
    return () => {
      active = false;
    };
  }, []);

  if (!isAvailable) {
    return null;
  }

  return (
    <section className="settings-panel" aria-labelledby="settings-developer-title">
      <div className="settings-panel-heading">
        <h2 id="settings-developer-title">Developer tools</h2>
        <p>Manage local skill packages while the development API is enabled.</p>
      </div>
      <button
        className="settings-primary-action settings-developer-action"
        onClick={onOpenDeveloperTools}
        type="button"
      >
        <Wrench size={16} aria-hidden="true" />
        Open developer tools
      </button>
    </section>
  );
}
```

Modify `Settings.tsx` props:

```tsx
type SettingsProps = {
  onBack: () => void;
  onOpenDeveloperTools: () => void;
};
```

Render:

```tsx
<SettingsDeveloperTools onOpenDeveloperTools={onOpenDeveloperTools} />
```

- [ ] **Step 5: Run route tests to verify they pass**

Run:

```bash
cd apps/desktop && pixi run npm test -- developer-tools.test.tsx
```

Expected: PASS.

- [ ] **Step 6: Commit Task 5**

Run:

```bash
git add apps/desktop/src/renderer/App.tsx apps/desktop/src/renderer/screens/Settings.tsx apps/desktop/src/renderer/components/SettingsDeveloperTools.tsx apps/desktop/tests/developer-tools.test.tsx
git diff --cached --name-only
git commit -m "feat: route developer skill tools"
```

Expected staged files:

```text
apps/desktop/src/renderer/App.tsx
apps/desktop/src/renderer/screens/Settings.tsx
apps/desktop/src/renderer/components/SettingsDeveloperTools.tsx
apps/desktop/tests/developer-tools.test.tsx
```

---

### Task 6: Visual Verification Against Stitch

**Files:**
- Modify only if screenshot review finds a concrete mismatch in Task 4 or Task 5 files.

**Interfaces:**
- Consumes: implemented `#developer` route, Stitch desktop and mobile screens.
- Produces: verified implementation screenshots and documented deviations for final delivery.

- [ ] **Step 1: Start dev services with dev API enabled**

Run:

```bash
GENERAL_AGENT_DEV_API=1 pixi run dev
```

Expected: server listens on `http://127.0.0.1:49321` and Vite serves the desktop renderer at `http://127.0.0.1:5173`.

- [ ] **Step 2: Open desktop developer route with Browser skill**

Use the Browser skill to navigate to:

```text
http://127.0.0.1:5173/#developer
```

Set viewport to `1280x900`. Capture a screenshot.

Expected:

- Header title is `Developer Tools`.
- Two-pane workbench appears.
- `echo` is listed and selected.
- `SKILL.md missing` appears as informational text.
- The detail pane resembles Stitch desktop screen `490091d713474aa784d1b42ce510af7b`.

- [ ] **Step 3: Open mobile developer route with Browser skill**

Use the Browser skill to navigate to:

```text
http://127.0.0.1:5173/#developer
```

Set viewport to `390x844`. Capture a screenshot.

Expected:

- Header actions fit without overlap.
- Package list is single-column.
- Selected `echo` details appear below the list.
- Action buttons have stable touch-friendly heights.
- The layout resembles Stitch mobile screen `333c3ec8116c4d7799f16c12d2d8dcdb`.

- [ ] **Step 4: Fix concrete visual mismatches**

If visual review shows overlap, wrong responsive layout, missing actions, one-note palette drift, or text overflow, patch only the relevant frontend files from Task 4 or Task 5 and rerun:

```bash
cd apps/desktop && pixi run npm test -- developer-tools.test.tsx
```

Expected: PASS after each visual fix.

- [ ] **Step 5: Stop dev services**

Stop the `pixi run dev` process cleanly with `Ctrl-C`.

Expected: no required dev server session remains running.

---

### Task 7: Full Verification And Final Commit

**Files:**
- Verify all files modified by Tasks 1-6.

**Interfaces:**
- Consumes: all previous task outputs.
- Produces: final tested development skill tools feature.

- [ ] **Step 1: Run Rust format**

Run:

```bash
pixi run fmt
```

Expected: command exits `0`.

- [ ] **Step 2: Run Rust tests**

Run:

```bash
pixi run test
```

Expected: command exits `0`.

- [ ] **Step 3: Run frontend tests**

Run:

```bash
cd apps/desktop && pixi run npm test
```

Expected: command exits `0`.

- [ ] **Step 4: Check source file line counts**

Run:

```bash
wc -l crates/agent-server/src/dev_skills.rs apps/desktop/src/renderer/screens/DeveloperTools.tsx apps/desktop/src/renderer/components/developer/*.tsx apps/desktop/src/renderer/styles/developer.css apps/desktop/tests/developer-tools.test.tsx
```

Expected: each listed source-like file is under `1000` lines.

- [ ] **Step 5: Review git diff**

Run:

```bash
git status --short
git diff --stat
```

Expected: only intended feature files are modified or added by this implementation. Pre-existing unrelated worktree changes remain untouched.

- [ ] **Step 6: Final commit if Task 6 produced fixes after the last task commit**

If Task 6 changed files after Task 5, run:

```bash
git add crates/agent-runtime/src/skill.rs crates/agent-runtime/src/skill_catalog.rs crates/agent-server/src/dev_skills.rs crates/agent-server/src/api.rs crates/agent-server/src/dev_api.rs crates/agent-server/src/main.rs apps/desktop/src/renderer/api.ts apps/desktop/src/renderer/devSkillPrompts.ts apps/desktop/src/renderer/App.tsx apps/desktop/src/renderer/screens/Settings.tsx apps/desktop/src/renderer/screens/DeveloperTools.tsx apps/desktop/src/renderer/components/SettingsDeveloperTools.tsx apps/desktop/src/renderer/components/developer apps/desktop/src/renderer/styles/index.css apps/desktop/src/renderer/styles/developer.css apps/desktop/tests/developer-tools.test.tsx
git commit -m "fix: align developer skill tools with stitch"
```

Expected: commit succeeds only when there are staged changes from visual fixes.

- [ ] **Step 7: Prepare final implementation summary**

Final response must include:

- Backend dev routes implemented.
- Frontend `#developer` route implemented.
- Stitch project and screen IDs used.
- Viewport sizes checked: `1280x900`, `390x844`.
- Visual review result and any accepted deviations.
- Test commands run and results.
