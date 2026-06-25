# Developer Agent Framework Repositioning Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reposition GeneralAgent as a developer-facing agent framework where developers author skills during development, packaged apps freeze skills as hidden runtime capabilities, and end users never manage skills directly.

**Architecture:** Keep the existing Rust runtime and Electron/React shell, but split skill behavior into development scanning and packaged frozen inventory. Production API and desktop UI should expose chat and model settings only; skill inventory remains internal runtime state.

**Tech Stack:** Rust 2024, Axum, Tokio, React 18, TypeScript, Vite, Vitest, Radix Dialog, lucide-react, pixi.

---

## Source Design

Use the confirmed repositioning spec:

- `docs/superpowers/specs/2026-06-25-developer-agent-framework-repositioning-design.md`

Use this Stitch project and design system for UI work:

- Stitch project: `projects/8616130577965446903`
- Design system: `assets/e4d441befa1d42e4af22f64b6d8e5d3c`

Generated UI source screens:

| View | Device | Stitch screen ID | Implementation target |
| --- | --- | --- | --- |
| Chat | Desktop | `d9113e7a1ce640c88135dcd875982cf0` | `Chat.tsx` title area copy |
| Chat | Mobile | `f74d11de4aa845e0bca5ac976c50352f` | responsive chat copy and no skill exposure |
| Settings Model | Desktop | `b591242868d74b0093a7f11b2c0c0f8e` | `Settings.tsx` single model settings view |
| Settings Model | Mobile | `0a239471a02d413da7880f4ccef955e6` | responsive settings without tabs |

Visual QC verdict: PASS. A fresh subagent reviewed all four Stitch screenshots and confirmed they do not expose skill/tool/capability/diagnostic concepts. Implementation must preserve that boundary, including hidden menus and status areas.

## File Structure

Modify these files:

- `crates/agent-runtime/src/skill.rs`
  - Owns development skill discovery, packaged skill bundle loading, manifest validation, and command execution.
- `crates/agent-server/src/api.rs`
  - Owns public production API routing. Add regression coverage that production routes do not expose `/skills` or `/dev/skills`.
- `apps/desktop/src/renderer/screens/Chat.tsx`
  - Owns visible chat title/subtitle. Remove skill-facing copy.
- `apps/desktop/src/renderer/screens/Settings.tsx`
  - Owns the settings route. Replace tabs with a single model connection surface.
- `apps/desktop/src/renderer/data/fixtures.ts`
  - Owns renderer mock chat and conversation data. Remove user-facing skill fixture data.
- `apps/desktop/src/renderer/types.ts`
  - Owns renderer-only types. Remove user-facing skill summary types.
- `apps/desktop/src/renderer/styles/settings.css`
  - Owns settings layout. Remove tabs and skill row styles.
- `apps/desktop/tests/chat.test.tsx`
  - Owns renderer behavior tests. Replace skill tab/toggle tests with hidden-skill UI assertions.
- `apps/desktop/package.json`
  - Remove unused Radix Tabs and Switch dependencies after deleting their imports.
- `apps/desktop/package-lock.json`
  - Update through `npm uninstall`, not by manual editing.
- `docs/feasibility.md`
  - Update product, skill, milestone, and API framing.
- `docs/mvp-verification.md`
  - Mark old Skills-tab verification as superseded by the new source-of-truth screens.
- `docs/superpowers/specs/2026-06-25-consumer-chat-ui-design.md`
  - Add a superseded notice that points to the new repositioning spec.
- `docs/superpowers/plans/2026-06-25-consumer-chat-ui.md`
  - Add a superseded notice that points to this plan and the new spec.

Delete this file:

- `apps/desktop/src/renderer/components/SettingsSkills.tsx`

Do not add user-facing skill APIs, a skill marketplace, or packaged-mode dynamic skill installation.

---

### Task 1: Split Development And Packaged Skill Loading

**Files:**
- Modify: `crates/agent-runtime/src/skill.rs`
- Test: `crates/agent-runtime/src/skill.rs`

- [ ] **Step 1: Write the failing packaged skill registry tests**

Append these tests inside the existing `#[cfg(test)] mod tests` in `crates/agent-runtime/src/skill.rs`, replacing the current single `loads_and_executes_echo_skill` test with the full test module content shown here:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn development_load_scans_skill_directories() {
        let skills_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join("skills");
        let registry = SkillRegistry::load_development(skills_root).await.unwrap();
        let tools = registry.tools();
        assert!(tools.iter().any(|tool| tool.name == "echo"));

        let result = registry
            .execute("echo", serde_json::json!({ "text": "hello" }))
            .await
            .unwrap();

        assert_eq!(result["text"], "hello");
    }

    #[tokio::test]
    async fn default_load_keeps_development_discovery_for_existing_callers() {
        let skills_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join("skills");
        let registry = SkillRegistry::load(skills_root).await.unwrap();

        assert!(registry.tools().iter().any(|tool| tool.name == "echo"));
    }

    #[tokio::test]
    async fn packaged_load_uses_only_the_frozen_skill_index() {
        let root = unique_test_dir("packaged-load");
        write_echo_skill(&root, "included", "included_echo").await;
        write_echo_skill(&root, "unlisted", "unlisted_echo").await;
        tokio::fs::write(
            root.join("skill-bundle.json"),
            serde_json::json!({
                "skills": [
                    { "path": "included" }
                ]
            })
            .to_string(),
        )
        .await
        .unwrap();

        let registry = SkillRegistry::load_packaged(&root).await.unwrap();
        let tool_names: Vec<_> = registry
            .tools()
            .into_iter()
            .map(|tool| tool.name)
            .collect();

        assert_eq!(tool_names, vec!["included_echo"]);
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn packaged_load_rejects_unsafe_index_paths() {
        let root = unique_test_dir("packaged-unsafe");
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(
            root.join("skill-bundle.json"),
            serde_json::json!({
                "skills": [
                    { "path": "../echo" }
                ]
            })
            .to_string(),
        )
        .await
        .unwrap();

        let error = SkillRegistry::load_packaged(&root).await.unwrap_err();

        assert!(error.to_string().contains("unsafe packaged skill path"));
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn rejects_manifest_without_runtime_tools() {
        let root = unique_test_dir("invalid-manifest");
        let skill_dir = root.join("empty");
        tokio::fs::create_dir_all(&skill_dir).await.unwrap();
        tokio::fs::write(
            skill_dir.join("skill.json"),
            serde_json::json!({
                "name": "empty",
                "description": "Invalid empty runtime skill.",
                "version": "0.1.0",
                "entry": {
                    "type": "command",
                    "command": "node",
                    "args": ["index.js"]
                },
                "tools": []
            })
            .to_string(),
        )
        .await
        .unwrap();

        let error = SkillRegistry::load_development(&root).await.unwrap_err();

        assert!(error.to_string().contains("must define at least one runtime tool"));
        remove_test_dir(root).await;
    }

    async fn write_echo_skill(root: &Path, folder: &str, tool_name: &str) {
        let skill_dir = root.join(folder);
        tokio::fs::create_dir_all(&skill_dir).await.unwrap();
        tokio::fs::write(
            skill_dir.join("skill.json"),
            serde_json::json!({
                "name": folder,
                "description": "Echo a text payload.",
                "version": "0.1.0",
                "entry": {
                    "type": "command",
                    "command": "node",
                    "args": ["index.js"]
                },
                "tools": [
                    {
                        "name": tool_name,
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
            })
            .to_string(),
        )
        .await
        .unwrap();
        tokio::fs::write(
            skill_dir.join("index.js"),
            "process.stdin.resume();\nprocess.stdin.on('data', (chunk) => process.stdout.write(chunk));\n",
        )
        .await
        .unwrap();
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "generalagent-{name}-{}",
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

- [ ] **Step 2: Run the new runtime tests and verify they fail**

Run:

```bash
pixi run cargo test -p agent-runtime skill::
```

Expected: FAIL because `SkillRegistry::load_development`, `SkillRegistry::load_packaged`, and packaged bundle validation do not exist yet.

- [ ] **Step 3: Implement development and packaged loading**

In `crates/agent-runtime/src/skill.rs`, update the imports and add these types near the existing manifest structs:

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SkillBundleIndex {
    pub skills: Vec<SkillBundleEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SkillBundleEntry {
    pub path: PathBuf,
}
```

Replace the current `impl SkillRegistry` with:

```rust
impl SkillRegistry {
    pub async fn load(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        Self::load_development(root).await
    }

    pub async fn load_development(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        let mut skills = Vec::new();
        let mut entries = tokio::fs::read_dir(root).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let manifest_path = path.join("skill.json");
            if !manifest_path.is_file() {
                continue;
            }

            skills.push(load_skill_at(path).await?);
        }

        Ok(Self { skills })
    }

    pub async fn load_packaged(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        let root = root.as_ref();
        let index_path = root.join("skill-bundle.json");
        let bytes = tokio::fs::read(&index_path).await?;
        let index: SkillBundleIndex = serde_json::from_slice(&bytes)?;
        let mut skills = Vec::new();

        for entry in index.skills {
            if !is_safe_relative_path(&entry.path) {
                anyhow::bail!(
                    "unsafe packaged skill path: {}",
                    entry.path.display()
                );
            }

            skills.push(load_skill_at(root.join(entry.path)).await?);
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
            .find(|skill| {
                skill
                    .manifest
                    .tools
                    .iter()
                    .any(|tool| tool.name == tool_name)
            })
            .ok_or_else(|| anyhow::anyhow!("unknown tool: {tool_name}"))?;

        let mut child = Command::new(&skill.manifest.entry.command)
            .args(&skill.manifest.entry.args)
            .current_dir(&skill.root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()?;

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("skill command stdin unavailable"))?;
        stdin
            .write_all(serde_json::to_vec(&input)?.as_slice())
            .await?;
        drop(stdin);

        let output = child.wait_with_output().await?;
        if !output.status.success() {
            anyhow::bail!("skill command failed: {}", output.status);
        }

        Ok(serde_json::from_slice(&output.stdout)?)
    }
}
```

Add these helper functions below the impl:

```rust
async fn load_skill_at(path: PathBuf) -> anyhow::Result<InstalledSkill> {
    let manifest_path = path.join("skill.json");
    let bytes = tokio::fs::read(&manifest_path).await?;
    let manifest: SkillManifest = serde_json::from_slice(&bytes)?;
    validate_manifest(&manifest)?;

    Ok(InstalledSkill {
        root: path,
        manifest,
    })
}

fn validate_manifest(manifest: &SkillManifest) -> anyhow::Result<()> {
    if manifest.name.trim().is_empty() {
        anyhow::bail!("skill manifest name is required");
    }

    if manifest.version.trim().is_empty() {
        anyhow::bail!("skill manifest version is required");
    }

    if manifest.entry.command.trim().is_empty() {
        anyhow::bail!("skill manifest entry command is required");
    }

    if manifest.tools.is_empty() {
        anyhow::bail!(
            "skill manifest '{}' must define at least one runtime tool",
            manifest.name
        );
    }

    let mut tool_names = HashSet::new();
    for tool in &manifest.tools {
        if tool.name.trim().is_empty() {
            anyhow::bail!("skill manifest '{}' has an empty tool name", manifest.name);
        }

        if !tool_names.insert(tool.name.clone()) {
            anyhow::bail!(
                "skill manifest '{}' defines duplicate tool '{}'",
                manifest.name,
                tool.name
            );
        }
    }

    Ok(())
}

fn is_safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path.components().all(|component| {
            matches!(component, Component::Normal(_))
        })
}
```

- [ ] **Step 4: Run runtime tests and verify they pass**

Run:

```bash
pixi run cargo test -p agent-runtime skill::
```

Expected: PASS for all skill registry tests.

- [ ] **Step 5: Run turn-loop regression test**

Run:

```bash
pixi run cargo test -p agent-runtime turn::tests::executes_tool_and_continues_until_text_response
```

Expected: PASS. This confirms the existing turn loop still works with the default development loader.

- [ ] **Step 6: Commit runtime split**

Run:

```bash
git add crates/agent-runtime/src/skill.rs
git commit -m "feat: add packaged skill registry loading"
```

---

### Task 2: Guard Production API Against Public Skill Inventory

**Files:**
- Modify: `crates/agent-server/src/api.rs`
- Test: `crates/agent-server/src/api.rs`

- [ ] **Step 1: Add production route guard test**

In `crates/agent-server/src/api.rs`, add this test inside `#[cfg(test)] mod tests` after `supports_vite_desktop_cors_preflight`:

```rust
#[tokio::test]
async fn production_router_does_not_expose_skill_inventory() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let app = router(Arc::new(AppState::new(storage)));

    for uri in ["/skills", "/dev/skills"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
```

- [ ] **Step 2: Run API tests**

Run:

```bash
pixi run cargo test -p agent-server api::tests::production_router_does_not_expose_skill_inventory
```

Expected: PASS. The current production router already has no skill inventory routes; this test freezes that boundary.

- [ ] **Step 3: Run full server API tests**

Run:

```bash
pixi run cargo test -p agent-server api::tests
```

Expected: PASS.

- [ ] **Step 4: Commit API guard**

Run:

```bash
git add crates/agent-server/src/api.rs
git commit -m "test: guard hidden production skills api"
```

---

### Task 3: Remove User-Facing Skill Settings From Desktop

**Files:**
- Modify: `apps/desktop/src/renderer/screens/Chat.tsx`
- Modify: `apps/desktop/src/renderer/screens/Settings.tsx`
- Modify: `apps/desktop/src/renderer/data/fixtures.ts`
- Modify: `apps/desktop/src/renderer/types.ts`
- Modify: `apps/desktop/src/renderer/styles/settings.css`
- Modify: `apps/desktop/tests/chat.test.tsx`
- Modify: `apps/desktop/package.json`
- Modify: `apps/desktop/package-lock.json`
- Delete: `apps/desktop/src/renderer/components/SettingsSkills.tsx`

- [ ] **Step 1: Update renderer tests first**

In `apps/desktop/tests/chat.test.tsx`, update the `exposes consumer chat controls` test to assert hidden skills copy:

```tsx
it("exposes consumer chat controls without skill-facing copy", () => {
  render(<Chat />);

  expect(
    screen.getByRole("button", { name: "Open conversations" })
  ).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Open settings" })).toBeInTheDocument();
  expect(
    screen.getByText("Ask naturally. The agent will handle the work.")
  ).toBeInTheDocument();
  expect(screen.queryByText(/use skills/i)).not.toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "Open sessions" })
  ).not.toBeInTheDocument();
});
```

Replace the three tests named `switches between model and skills settings`, `toggles an available skill`, and `keeps unavailable skills disabled` with these two tests:

```tsx
it("shows only model connection settings to end users", () => {
  window.history.replaceState(null, "", "/#settings");

  render(<App />);

  expect(screen.getByRole("heading", { name: "Settings" })).toBeInTheDocument();
  expect(screen.getByRole("heading", { name: "Model connection" })).toBeInTheDocument();
  expect(screen.getByLabelText("Base URL")).toBeInTheDocument();
  expect(screen.getByLabelText("API key")).toBeInTheDocument();
  expect(screen.getByLabelText("Model name")).toBeInTheDocument();
  expect(screen.queryByRole("tab", { name: "Skills" })).not.toBeInTheDocument();
  expect(screen.queryByText("File Helper")).not.toBeInTheDocument();
  expect(screen.queryByText("Web Research")).not.toBeInTheDocument();
});

it("keeps user-facing settings free of skill controls", () => {
  window.history.replaceState(null, "", "/#settings");

  render(<App />);

  expect(screen.queryByRole("switch")).not.toBeInTheDocument();
  expect(screen.queryByText(/skill/i)).not.toBeInTheDocument();
  expect(screen.queryByText(/tool/i)).not.toBeInTheDocument();
});
```

Replace the `scopes settings skill row styles away from session skill rows` test with:

```tsx
it("keeps settings styles free of user-facing skill selectors", () => {
  const css = readCssBundle("src/renderer/styles/index.css");

  expect(css).toMatch(/\.conversation-drawer-content[\s\S]*?\{/);
  expect(css).toMatch(/\.settings-shell[\s\S]*?\{/);
  expect(css).toMatch(/\.settings-panel[\s\S]*?\{/);
  expect(css).not.toMatch(/\.settings-skill-row/);
  expect(css).not.toMatch(/\.skill-switch/);
  expect(css).not.toMatch(/(^|[,{]\s*)\.skill-row(?:\s|,|\{)/m);
});
```

- [ ] **Step 2: Run renderer tests and verify they fail**

Run:

```bash
pixi run npm --prefix apps/desktop test
```

Expected: FAIL because chat still says "use skills", Settings still renders the Skills tab, skill fixtures still exist, and settings CSS still includes skill selectors.

- [ ] **Step 3: Replace Settings route with model-only surface**

Replace the full contents of `apps/desktop/src/renderer/screens/Settings.tsx` with:

```tsx
import { ArrowLeft } from "lucide-react";

import { AppIconButton } from "../components/AppIconButton";
import { SettingsModel } from "../components/SettingsModel";

type SettingsProps = {
  onBack: () => void;
};

export function Settings({ onBack }: SettingsProps): JSX.Element {
  return (
    <main className="settings-screen" aria-label="Settings">
      <header className="top-bar settings-top-bar">
        <AppIconButton label="Back to chat" onClick={onBack}>
          <ArrowLeft size={18} aria-hidden="true" />
        </AppIconButton>
        <div className="top-bar-title">
          <h1>Settings</h1>
        </div>
        <span className="top-bar-spacer" aria-hidden="true" />
      </header>
      <div className="settings-shell">
        <SettingsModel />
      </div>
    </main>
  );
}
```

- [ ] **Step 4: Update chat copy**

In `apps/desktop/src/renderer/screens/Chat.tsx`, replace:

```tsx
<p>Ask anything, use skills when you need them.</p>
```

with:

```tsx
<p>Ask naturally. The agent will handle the work.</p>
```

- [ ] **Step 5: Remove skill fixtures and types**

In `apps/desktop/src/renderer/data/fixtures.ts`, replace:

```tsx
import { ChatMessage, ConversationSummary, SkillSummary } from "../types";
```

with:

```tsx
import { ChatMessage, ConversationSummary } from "../types";
```

Delete the entire exported `skills` array from `apps/desktop/src/renderer/data/fixtures.ts`.

In `apps/desktop/src/renderer/types.ts`, delete:

```tsx
export type SkillStatus = "active" | "inactive" | "unavailable";

export type SkillSummary = {
  description: string;
  enabled: boolean;
  id: string;
  name: string;
  status: SkillStatus;
};
```

- [ ] **Step 6: Delete the Skills component**

Run:

```bash
rm apps/desktop/src/renderer/components/SettingsSkills.tsx
```

Expected: The file is removed. No other file should import it after Step 3.

- [ ] **Step 7: Remove unused Radix dependencies**

Run:

```bash
pixi run npm --prefix apps/desktop uninstall @radix-ui/react-tabs @radix-ui/react-switch
```

Expected: `apps/desktop/package.json` and `apps/desktop/package-lock.json` no longer list `@radix-ui/react-tabs` or `@radix-ui/react-switch`. `@radix-ui/react-dialog` remains because `ConversationDrawer.tsx` still uses it.

- [ ] **Step 8: Remove settings tab and skill CSS**

In `apps/desktop/src/renderer/styles/settings.css`, delete the full CSS blocks with these exact selectors:

```text
.settings-tabs
.settings-tab
.settings-tab:last-child
.settings-tab[data-state="active"]
.settings-tab-content
.settings-tab-content:focus-visible
.skill-list
.settings-skill-row
.skill-copy
.skill-title-row
.skill-title-row h3
.skill-status
.skill-switch
.skill-switch[data-state="checked"]
.skill-switch:disabled
.skill-switch-thumb
.skill-switch[data-state="checked"] .skill-switch-thumb
```

Replace this selector group:

```css
.settings-panel-heading h2,
.skill-title-row h3 {
  margin: 0;
}
```

with:

```css
.settings-panel-heading h2 {
  margin: 0;
}
```

Replace this selector group:

```css
.settings-panel-heading p,
.skill-copy p,
.settings-status {
  margin: 0;
  color: var(--color-text-muted);
}
```

with:

```css
.settings-panel-heading p,
.settings-status {
  margin: 0;
  color: var(--color-text-muted);
}
```

In the mobile media block, replace:

```css
.settings-actions,
.settings-skill-row {
  align-items: stretch;
  flex-direction: column;
}

.skill-switch {
  align-self: flex-start;
}
```

with:

```css
.settings-actions {
  align-items: stretch;
  flex-direction: column;
}
```

- [ ] **Step 9: Run renderer tests**

Run:

```bash
pixi run npm --prefix apps/desktop test
```

Expected: PASS.

- [ ] **Step 10: Run TypeScript check**

Run:

```bash
cd apps/desktop && pixi run npm exec tsc -- --noEmit -p tsconfig.vitest.json
```

Expected: PASS. This catches stale imports from deleted `SettingsSkills.tsx` and removed Radix packages.

- [ ] **Step 11: Commit desktop hidden-skill UI**

Run:

```bash
git add apps/desktop/package.json apps/desktop/package-lock.json apps/desktop/src/renderer apps/desktop/tests/chat.test.tsx
git add -u apps/desktop/src/renderer/components/SettingsSkills.tsx
git commit -m "fix: hide skills from packaged user settings"
```

---

### Task 4: Update Product And Verification Documentation

**Files:**
- Modify: `docs/feasibility.md`
- Modify: `docs/mvp-verification.md`
- Modify: `docs/superpowers/specs/2026-06-25-consumer-chat-ui-design.md`
- Modify: `docs/superpowers/plans/2026-06-25-consumer-chat-ui.md`

- [ ] **Step 1: Update feasibility positioning**

In `docs/feasibility.md`, update the opening recommendation section so it states:

```markdown
GeneralAgent 的产品定位调整为“为开发者服务的 agent 应用框架”。开发者在开发阶段通过本地 skill 包、Codex 内置 `skill-creator`、以及后续 SDK/脚手架扩展 agent 能力；打包后 skill inventory 被固定为应用内部能力，对终端用户不可见。用户只通过自然对话表达意图，由 runtime 自动选择和调用内置能力。
```

In the MVP scope section, replace any wording that implies users manage skills with:

```markdown
3. 用户发送信息后，agent 可以循环思考、自动调用打包内置 skill/tool、继续请求模型，直到回复用户。
```

In the Skill system section, add this paragraph before the manifest example:

```markdown
Skill 是开发者扩展点，不是终端用户配置项。开发模式可以扫描 `skills/` 目录并支持调试诊断；打包模式必须读取冻结的 skill bundle/index。生产 UI 和生产 API 默认不暴露 skill 列表、开关或 marketplace。
```

In the milestones, replace:

```markdown
- Skill management screen。
```

with:

```markdown
- Model profile settings。
- 不提供终端用户 skill management screen；后续只在 dev mode 增加 skill validation/diagnostics。
```

In the API list, replace:

```markdown
5. `GET /skills`
6. `POST /model-profiles`
```

with:

```markdown
5. `POST /model-profiles`
6. Dev-only: `GET /dev/skills` / `POST /dev/skills/validate`，生产包默认关闭。
```

- [ ] **Step 2: Mark historical consumer UI spec as superseded**

At the top of `docs/superpowers/specs/2026-06-25-consumer-chat-ui-design.md`, after the title and date, add:

```markdown
> Superseded note: The user-facing Skills settings direction in this document has been replaced by `docs/superpowers/specs/2026-06-25-developer-agent-framework-repositioning-design.md`. GeneralAgent is now positioned as a developer-facing agent framework. Packaged apps hide skills from end users and use them automatically at runtime.
```

- [ ] **Step 3: Mark historical consumer UI plan as superseded**

At the top of `docs/superpowers/plans/2026-06-25-consumer-chat-ui.md`, after the required sub-skill header, add:

```markdown
> Superseded note: The Skills settings tab described in this plan is no longer a valid product target. Use `docs/superpowers/specs/2026-06-25-developer-agent-framework-repositioning-design.md` and `docs/superpowers/plans/2026-06-25-developer-agent-framework-repositioning.md` for the current implementation direction.
```

- [ ] **Step 4: Update MVP verification notes**

In `docs/mvp-verification.md`, add this note under `## Consumer Chat UI Verification`:

```markdown
> Repositioning note: The Settings Skills desktop/mobile checks below describe a superseded implementation. The current target is model-only Settings, sourced from Stitch screens `b591242868d74b0093a7f11b2c0c0f8e` and `0a239471a02d413da7880f4ccef955e6`, with no user-facing skill controls.
```

In the visual review result section, add:

```markdown
- Repositioning follow-up required: remove user-facing Skills tab and verify model-only Settings at desktop and mobile breakpoints.
```

- [ ] **Step 5: Run documentation consistency search**

Run:

```bash
rg -n "Skill management screen|GET /skills|Skills tab|skill toggle|use skills when you need them" docs apps crates
```

Expected: matches remain only in superseded historical notes, this implementation plan, or old verification text explicitly marked as superseded. No active product direction should instruct user-facing skill management.

- [ ] **Step 6: Commit documentation updates**

Run:

```bash
git add docs/feasibility.md docs/mvp-verification.md docs/superpowers/specs/2026-06-25-consumer-chat-ui-design.md docs/superpowers/plans/2026-06-25-consumer-chat-ui.md
git commit -m "docs: hide packaged skills from users"
```

---

### Task 5: Full Verification And Visual Check

**Files:**
- Read/check: all modified files from Tasks 1-4
- Update if needed: `docs/mvp-verification.md`

- [ ] **Step 1: Run Rust tests**

Run:

```bash
pixi run cargo test --workspace
```

Expected: PASS for `model-gateway`, `agent-runtime`, and `agent-server`.

- [ ] **Step 2: Run Rust clippy**

Run:

```bash
pixi run cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS with no warnings.

- [ ] **Step 3: Run renderer tests**

Run:

```bash
pixi run npm --prefix apps/desktop test
```

Expected: PASS.

- [ ] **Step 4: Run renderer TypeScript check**

Run:

```bash
cd apps/desktop && pixi run npm exec tsc -- --noEmit -p tsconfig.vitest.json
```

Expected: PASS.

- [ ] **Step 5: Check source line budget**

Run:

```bash
wc -l crates/agent-runtime/src/skill.rs crates/agent-server/src/api.rs apps/desktop/src/renderer/screens/Settings.tsx apps/desktop/src/renderer/screens/Chat.tsx apps/desktop/src/renderer/data/fixtures.ts apps/desktop/src/renderer/types.ts apps/desktop/src/renderer/styles/settings.css apps/desktop/tests/chat.test.tsx
```

Expected: every edited source file is under 1000 physical lines.

- [ ] **Step 6: Run whitespace check**

Run:

```bash
git diff --check HEAD
```

Expected: no output and exit code 0.

- [ ] **Step 7: Start local desktop dev server**

Run:

```bash
pixi run npm --prefix apps/desktop run dev -- --host 127.0.0.1 --port 5173
```

If port 5173 is occupied, use:

```bash
pixi run npm --prefix apps/desktop run dev -- --host 127.0.0.1 --port 5174
```

Expected: Vite serves the app locally.

- [ ] **Step 8: Manual browser check against Stitch**

Use the Browser or Computer Use skill to inspect these routes and breakpoints:

```text
http://127.0.0.1:5173/            desktop 1440x900
http://127.0.0.1:5173/            mobile 390x844
http://127.0.0.1:5173/#settings  desktop 1440x900
http://127.0.0.1:5173/#settings  mobile 390x844
```

Expected:

- Chat subtitle matches `Ask naturally. The agent will handle the work.`
- Chat contains no visible `skill`, `tool`, `capability`, `diagnostic`, or marketplace UI.
- Settings contains only Model connection.
- Settings contains no tabs.
- Settings contains no Skills tab.
- Settings contains no switches.
- Mobile Settings text fits and does not overlap.
- Layout visually tracks Stitch screens `d9113e7a1ce640c88135dcd875982cf0`, `f74d11de4aa845e0bca5ac976c50352f`, `b591242868d74b0093a7f11b2c0c0f8e`, and `0a239471a02d413da7880f4ccef955e6`.

- [ ] **Step 9: Update verification notes**

If manual checks pass, append this note to `docs/mvp-verification.md`:

```markdown
## Developer Framework Repositioning Verification

Date: 2026-06-25

Stitch source of truth:

- Project: `projects/8616130577965446903`
- Design system: `assets/e4d441befa1d42e4af22f64b6d8e5d3c`
- Chat desktop: `d9113e7a1ce640c88135dcd875982cf0`
- Chat mobile: `f74d11de4aa845e0bca5ac976c50352f`
- Settings desktop: `b591242868d74b0093a7f11b2c0c0f8e`
- Settings mobile: `0a239471a02d413da7880f4ccef955e6`

Visual result: PASS. The packaged user UI exposes chat and model connection settings only. Skills remain hidden runtime capabilities and are not presented as user-managed settings.
```

- [ ] **Step 10: Commit verification update**

Run:

```bash
git add docs/mvp-verification.md
git commit -m "docs: verify hidden packaged skills ui"
```

---

## Final Review Checklist

Before reporting completion:

- [ ] `SkillRegistry::load_development` scans local skill directories for development.
- [ ] `SkillRegistry::load_packaged` reads only `skill-bundle.json`.
- [ ] Packaged skill paths reject absolute paths and parent traversal.
- [ ] Runtime skill manifests reject empty tool lists.
- [ ] Production API has no `/skills` or `/dev/skills` route.
- [ ] Desktop Settings has no Skills tab.
- [ ] Desktop Settings has no skill toggles.
- [ ] Chat copy no longer says users can use skills.
- [ ] `@radix-ui/react-tabs` and `@radix-ui/react-switch` are removed.
- [ ] Documentation clearly states `skill-creator` is a development authoring path.
- [ ] Documentation clearly states production packages hide skill inventory from users.
- [ ] All edited source files are under 1000 physical lines.
- [ ] Automated tests and manual UI checks pass.
