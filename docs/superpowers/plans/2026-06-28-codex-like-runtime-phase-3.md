# Codex-Like Runtime Phase 3 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build Phase 3 of the Codex-like runtime: a Codex-style `SKILL.md` instruction catalog that can discover skill summaries, inject available-skill context, and load full skill instructions only when triggered.

**Architecture:** Keep `SkillRegistry` as the runtime `skill.json` function-tool loader and add a separate `SkillCatalog` for instruction skills. `InstructionContext` receives skill summaries and selected full instruction blocks from the catalog, while `TurnRunner` owns the trigger decision for the current user message. Packaged mode excludes instruction skills unless the packaged manifest explicitly opts in.

**Tech Stack:** Rust 2024, Tokio async filesystem APIs, serde/serde_json, existing `InstructionContext` and `TurnRunner`, pixi-managed cargo commands.

---

## Scope

Phase 3 scope from the migration design:

- Add `SkillCatalog`.
- Parse `SKILL.md` front matter fields such as `name`, `description`, and optional `aliases`.
- Discover available skill summaries from configured skill roots.
- Inject available skill summaries into model-visible developer context.
- Load full `SKILL.md` instructions only for explicitly or uniquely triggered skills.
- Keep compatibility with existing `skill.json` runtime tools.
- Allow instruction-only skills without `skill.json`.
- Allow runtime-only tools without `SKILL.md`.
- Exclude `SKILL.md` in packaged mode unless `skill-bundle.json` explicitly configures inclusion.

Out of scope:

- New default filesystem tools.
- MCP, connectors, deferred tools, and plugin installation.
- Approval prompts or stronger sandbox profiles.
- Deep `references/` routing. Phase 3 keeps referenced files as future work unless they are included directly in `SKILL.md`.
- Desktop UI changes.

## Current Context

- Work on `main`, per repository instructions. Do not create a worktree.
- Current clean baseline after `d0a2f30 feat: finalize model settings and skill validation` has only untracked `.codex/`, which must remain untracked unless explicitly requested.
- Existing Phase 1/2 files:
  - `crates/agent-runtime/src/instructions.rs` builds system/developer/user model input and injects AGENTS.md blocks.
  - `crates/agent-runtime/src/skill.rs` loads and executes `skill.json` runtime tools.
  - `crates/agent-runtime/src/turn.rs` builds `InstructionContext`, advertises runtime tools, executes tool calls, and continues the loop.
  - `crates/agent-runtime/src/tools/mod.rs` owns `RuntimeConfig`.
- Source files must remain under 1000 physical lines. Add new modules for the catalog and keep new tests focused.

## File Structure

Create:

- `crates/agent-runtime/src/skill_catalog.rs`
  - Owns instruction skill metadata, front matter parsing, development discovery, packaged discovery, trigger selection, full instruction loading, and tests.

Modify:

- `crates/agent-runtime/src/lib.rs`
  - Export `skill_catalog`.
- `crates/agent-runtime/src/instructions.rs`
  - Add optional skill summaries and selected skill instruction documents to `InstructionConfig`.
  - Render deterministic `<available_skills>` and `<skill_instructions>` blocks before the user message.
- `crates/agent-runtime/src/turn.rs`
  - Add a `SkillCatalog` field to `TurnRunner`.
  - Provide constructors that default to an empty catalog for existing callers.
  - Add `new_with_catalog_and_config` for tests and server wiring.
  - Select triggered skill instructions from the current user text before building `InstructionContext`.
- `crates/agent-server/src/main.rs`
  - Load a development `SkillCatalog` from the same skills root used by `SkillRegistry`.
- `docs/mvp-verification.md`
  - Append Phase 3 verification evidence after final verification passes.

No desktop UI files should change in Phase 3.

## Data Shapes

Skill summary block:

```text
<available_skills count="2">
- name: filesystem
  description: Work with workspace files safely.
  source: filesystem/SKILL.md
- name: planning
  description: Write implementation plans.
  source: planning/SKILL.md
</available_skills>
```

Full skill block:

```text
<skill_instructions name="planning" source="planning/SKILL.md" bytes="2048" original_bytes="2048" truncated="false">
# Planning
Use checklists.
</skill_instructions>
```

Packaged instruction manifest entry:

```json
{
  "path": "included",
  "include_instructions": true
}
```

If `include_instructions` is missing or false, `SkillCatalog::load_packaged` must not load `included/SKILL.md`.

## Task 1: Add `SkillCatalog` Front Matter Parsing

**Files:**
- Create: `crates/agent-runtime/src/skill_catalog.rs`
- Modify: `crates/agent-runtime/src/lib.rs`

- [ ] **Step 1: Write failing parser tests**

Add this initial module and tests to `crates/agent-runtime/src/skill_catalog.rs`:

```rust
use anyhow::Context;
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub source: PathBuf,
}

#[derive(Clone, Debug)]
pub struct SkillCatalog {
    root: Option<PathBuf>,
    summaries: Vec<SkillSummary>,
}

#[derive(Debug, Deserialize)]
struct SkillFrontMatter {
    name: String,
    description: String,
    #[serde(default)]
    aliases: Vec<String>,
}

impl SkillCatalog {
    pub fn empty() -> Self {
        Self {
            root: None,
            summaries: Vec::new(),
        }
    }

    pub fn summaries(&self) -> &[SkillSummary] {
        &self.summaries
    }
}

fn parse_skill_front_matter(content: &str) -> anyhow::Result<SkillFrontMatter> {
    let _ = content;
    anyhow::bail!("not implemented")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_skill_front_matter_with_aliases() {
        let front_matter = parse_skill_front_matter(
            r#"---
name: planning
description: Write implementation plans.
aliases:
  - plan-writer
  - planner
---

# Planning
"#,
        )
        .unwrap();

        assert_eq!(front_matter.name, "planning");
        assert_eq!(front_matter.description, "Write implementation plans.");
        assert_eq!(front_matter.aliases, vec!["plan-writer", "planner"]);
    }

    #[test]
    fn rejects_skill_without_front_matter() {
        let error = parse_skill_front_matter("# Missing metadata").unwrap_err();

        assert!(error.to_string().contains("front matter"));
    }

    #[test]
    fn rejects_skill_with_empty_description() {
        let error = parse_skill_front_matter(
            r#"---
name: empty-description
description: ""
---

# Empty
"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("description"));
    }
}
```

Add this export to `crates/agent-runtime/src/lib.rs`:

```rust
pub mod skill_catalog;
```

- [ ] **Step 2: Run parser tests to verify they fail**

Run:

```bash
pixi run cargo test -p agent-runtime skill_catalog::tests -- --nocapture
```

Expected: tests fail because `parse_skill_front_matter` is not implemented.

- [ ] **Step 3: Implement minimal front matter parser**

Replace `parse_skill_front_matter` with a small deterministic parser that supports the YAML subset needed by Codex-style skill metadata:

```rust
fn parse_skill_front_matter(content: &str) -> anyhow::Result<SkillFrontMatter> {
    let Some(rest) = content.strip_prefix("---\n") else {
        anyhow::bail!("SKILL.md must start with front matter");
    };
    let Some((front_matter, _body)) = rest.split_once("\n---") else {
        anyhow::bail!("SKILL.md front matter must be closed");
    };

    let mut name: Option<String> = None;
    let mut description: Option<String> = None;
    let mut aliases = Vec::new();
    let mut in_aliases = false;

    for raw_line in front_matter.lines() {
        let line = raw_line.trim_end();
        if line.trim().is_empty() {
            continue;
        }

        if let Some(value) = line.strip_prefix("name:") {
            name = Some(unquote_scalar(value.trim()));
            in_aliases = false;
        } else if let Some(value) = line.strip_prefix("description:") {
            description = Some(unquote_scalar(value.trim()));
            in_aliases = false;
        } else if line.trim() == "aliases:" {
            in_aliases = true;
        } else if in_aliases {
            let trimmed = line.trim_start();
            let Some(value) = trimmed.strip_prefix("- ") else {
                anyhow::bail!("unsupported aliases entry in SKILL.md front matter");
            };
            aliases.push(unquote_scalar(value.trim()));
        }
    }

    let name = name
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("SKILL.md front matter name must not be empty"))?;
    let description = description
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("SKILL.md front matter description must not be empty"))?;

    Ok(SkillFrontMatter {
        name,
        description,
        aliases,
    })
}

fn unquote_scalar(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|inner| inner.strip_suffix('\'')))
        .unwrap_or(value)
        .to_string()
}
```

- [ ] **Step 4: Run parser tests to verify they pass**

Run:

```bash
pixi run cargo test -p agent-runtime skill_catalog::tests -- --nocapture
```

Expected: parser tests pass.

- [ ] **Step 5: Commit parser task**

Run:

```bash
git add crates/agent-runtime/src/skill_catalog.rs crates/agent-runtime/src/lib.rs
git commit -m "feat: add skill catalog front matter parser"
```

## Task 2: Discover Development and Packaged Instruction Skills

**Files:**
- Modify: `crates/agent-runtime/src/skill_catalog.rs`

- [ ] **Step 1: Write failing discovery tests**

Add these tests:

```rust
#[tokio::test]
async fn development_catalog_discovers_instruction_only_skill() {
    let root = unique_test_dir("development-catalog");
    write_skill_md(
        &root,
        "planning",
        r#"---
name: planning
description: Write plans.
---

# Planning
"#,
    )
    .await;

    let catalog = SkillCatalog::load_development(&root).await.unwrap();

    assert_eq!(catalog.summaries()[0].name, "planning");
    assert_eq!(catalog.summaries()[0].description, "Write plans.");
    assert_eq!(catalog.summaries()[0].source, PathBuf::from("planning/SKILL.md"));
    remove_test_dir(root).await;
}

#[tokio::test]
async fn packaged_catalog_includes_only_opted_in_skill_instructions() {
    let root = unique_test_dir("packaged-catalog");
    write_skill_md(
        &root,
        "included",
        r#"---
name: included
description: Included instructions.
---

# Included
"#,
    )
    .await;
    write_skill_md(
        &root,
        "excluded",
        r#"---
name: excluded
description: Excluded instructions.
---

# Excluded
"#,
    )
    .await;
    tokio::fs::write(
        root.join("skill-bundle.json"),
        serde_json::json!({
            "skills": [
                { "path": "included", "include_instructions": true },
                { "path": "excluded" }
            ]
        })
        .to_string(),
    )
    .await
    .unwrap();

    let catalog = SkillCatalog::load_packaged(&root).await.unwrap();
    let names: Vec<_> = catalog
        .summaries()
        .iter()
        .map(|summary| summary.name.as_str())
        .collect();

    assert_eq!(names, vec!["included"]);
    remove_test_dir(root).await;
}
```

- [ ] **Step 2: Run discovery tests to verify they fail**

Run:

```bash
pixi run cargo test -p agent-runtime skill_catalog::tests -- --nocapture
```

Expected: tests fail because loading functions do not exist.

- [ ] **Step 3: Implement development and packaged discovery**

Add these types and functions:

```rust
#[derive(Debug, Deserialize)]
struct SkillBundleIndex {
    skills: Vec<SkillBundleEntry>,
}

#[derive(Debug, Deserialize)]
struct SkillBundleEntry {
    path: PathBuf,
    #[serde(default)]
    include_instructions: bool,
}

impl SkillCatalog {
    pub async fn load_development(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        let root = root.as_ref();
        let canonical_root = tokio::fs::canonicalize(root)
            .await
            .with_context(|| format!("failed to resolve skill root {}", root.display()))?;
        let mut summaries = Vec::new();
        let mut entries = tokio::fs::read_dir(root).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let skill_path = path.join("SKILL.md");
            if skill_path.is_file() {
                summaries.push(read_skill_summary(&canonical_root, &skill_path).await?);
            }
        }

        summaries.sort_by(|left, right| left.name.cmp(&right.name));
        validate_unique_skill_names(&summaries)?;
        Ok(Self {
            root: Some(canonical_root),
            summaries,
        })
    }

    pub async fn load_packaged(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        let root = root.as_ref();
        let canonical_root = tokio::fs::canonicalize(root)
            .await
            .with_context(|| format!("failed to resolve packaged skill root {}", root.display()))?;
        let bytes = tokio::fs::read(root.join("skill-bundle.json"))
            .await
            .with_context(|| format!("failed to read packaged skill index in {}", root.display()))?;
        let index: SkillBundleIndex = serde_json::from_slice(&bytes)?;
        let mut summaries = Vec::new();

        for entry in index.skills {
            if !entry.include_instructions {
                continue;
            }
            let skill_root = resolve_safe_catalog_path(root, &canonical_root, &entry.path).await?;
            let skill_path = skill_root.join("SKILL.md");
            if skill_path.is_file() {
                summaries.push(read_skill_summary(&canonical_root, &skill_path).await?);
            }
        }

        summaries.sort_by(|left, right| left.name.cmp(&right.name));
        validate_unique_skill_names(&summaries)?;
        Ok(Self {
            root: Some(canonical_root),
            summaries,
        })
    }
}
```

Add safe path, summary, and test helper implementations in the same module. Reuse the packaged path safety rules from `skill.rs`: relative, normal components only, canonical result must stay under root.

- [ ] **Step 4: Run discovery tests to verify they pass**

Run:

```bash
pixi run cargo test -p agent-runtime skill_catalog::tests -- --nocapture
```

Expected: discovery and parser tests pass.

- [ ] **Step 5: Commit discovery task**

Run:

```bash
git add crates/agent-runtime/src/skill_catalog.rs
git commit -m "feat: discover instruction skill catalog"
```

## Task 3: Render Skill Summaries and Full Skill Instructions

**Files:**
- Modify: `crates/agent-runtime/src/skill_catalog.rs`
- Modify: `crates/agent-runtime/src/instructions.rs`

- [ ] **Step 1: Write failing injection tests**

Add a unit test in `crates/agent-runtime/src/instructions.rs`:

```rust
#[test]
fn renders_skill_summaries_and_selected_skill_instructions() {
    let root = temp_workspace();
    let mut config = InstructionConfig::new(&root, &root);
    config.skill_summaries = vec![crate::skill_catalog::SkillSummary {
        name: "planning".into(),
        description: "Write implementation plans.".into(),
        aliases: vec!["planner".into()],
        source: PathBuf::from("planning/SKILL.md"),
    }];
    config.skill_instructions = vec![SkillInstructionDocument {
        name: "planning".into(),
        source: PathBuf::from("planning/SKILL.md"),
        content: "# Planning\nUse checklists.".into(),
        truncated: false,
        read_bytes: 25,
        original_bytes: 25,
    }];

    let context = InstructionContext::load(config).unwrap();
    let input = context.model_input("use $planning");
    let developer = input[1]["content"].as_str().unwrap();

    assert!(developer.contains("<available_skills count=\"1\">"));
    assert!(developer.contains("name: planning"));
    assert!(developer.contains("aliases: planner"));
    assert!(developer.contains("<skill_instructions name=\"planning\""));
    assert!(developer.contains("# Planning"));
}
```

Add a catalog test in `crates/agent-runtime/src/skill_catalog.rs`:

```rust
#[tokio::test]
async fn loads_full_instruction_for_triggered_skill_with_truncation_metadata() {
    let root = unique_test_dir("load-full-instruction");
    write_skill_md(
        &root,
        "planning",
        r#"---
name: planning
description: Write plans.
---

# Planning
Use checklists.
"#,
    )
    .await;
    let catalog = SkillCatalog::load_development(&root).await.unwrap();

    let instructions = catalog
        .load_instruction_documents(&["planning".to_string()], 32)
        .await
        .unwrap();

    assert_eq!(instructions[0].name, "planning");
    assert_eq!(instructions[0].source, PathBuf::from("planning/SKILL.md"));
    assert!(instructions[0].content.contains("---"));
    assert!(instructions[0].truncated);
    remove_test_dir(root).await;
}
```

- [ ] **Step 2: Run injection tests to verify they fail**

Run:

```bash
pixi run cargo test -p agent-runtime instructions::tests::renders_skill_summaries_and_selected_skill_instructions skill_catalog::tests::loads_full_instruction_for_triggered_skill_with_truncation_metadata
```

Expected: tests fail because instruction data fields and full loading are missing.

- [ ] **Step 3: Implement instruction documents and rendering**

Add this public document type to `skill_catalog.rs`:

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillInstructionDocument {
    pub name: String,
    pub source: PathBuf,
    pub content: String,
    pub truncated: bool,
    pub read_bytes: usize,
    pub original_bytes: usize,
}
```

Add `load_instruction_documents(&self, names: &[String], max_instruction_bytes: usize)` that finds summaries by name, reads the matching `SKILL.md` under `self.root`, caps by `max_instruction_bytes`, and returns deterministic documents in the order requested. If `self.root` is `None`, return an error stating that the catalog has no filesystem root.

Modify `InstructionConfig`:

```rust
pub skill_summaries: Vec<crate::skill_catalog::SkillSummary>,
pub skill_instructions: Vec<crate::skill_catalog::SkillInstructionDocument>,
```

Initialize both to empty in `InstructionConfig::new`.

Render summaries before selected full instructions in `developer_context`.

- [ ] **Step 4: Run injection tests to verify they pass**

Run:

```bash
pixi run cargo test -p agent-runtime instructions::tests::renders_skill_summaries_and_selected_skill_instructions skill_catalog::tests::loads_full_instruction_for_triggered_skill_with_truncation_metadata
```

Expected: both tests pass.

- [ ] **Step 5: Commit rendering task**

Run:

```bash
git add crates/agent-runtime/src/skill_catalog.rs crates/agent-runtime/src/instructions.rs
git commit -m "feat: inject instruction skill context"
```

## Task 4: Trigger Skill Instructions in the Turn Loop

**Files:**
- Modify: `crates/agent-runtime/src/skill_catalog.rs`
- Modify: `crates/agent-runtime/src/turn.rs`
- Modify: `crates/agent-server/src/main.rs`

- [ ] **Step 1: Write failing trigger tests**

Add tests to `skill_catalog.rs`:

```rust
#[test]
fn trigger_policy_matches_explicit_dollar_skill() {
    let catalog = SkillCatalog {
        root: None,
        summaries: vec![SkillSummary {
            name: "planning".into(),
            description: "Write plans.".into(),
            aliases: vec![],
            source: PathBuf::from("planning/SKILL.md"),
        }],
    };

    assert_eq!(
        catalog.triggered_skill_names("please use $planning"),
        vec!["planning".to_string()]
    );
}

#[test]
fn trigger_policy_matches_unique_plain_text_name_or_alias() {
    let catalog = SkillCatalog {
        root: None,
        summaries: vec![
            SkillSummary {
                name: "planning".into(),
                description: "Write plans.".into(),
                aliases: vec!["planner".into()],
                source: PathBuf::from("planning/SKILL.md"),
            },
            SkillSummary {
                name: "debugging".into(),
                description: "Debug issues.".into(),
                aliases: vec![],
                source: PathBuf::from("debugging/SKILL.md"),
            },
        ],
    };

    assert_eq!(
        catalog.triggered_skill_names("planner should help"),
        vec!["planning".to_string()]
    );
}

#[test]
fn trigger_policy_ignores_ambiguous_plain_text_mentions() {
    let catalog = SkillCatalog {
        root: None,
        summaries: vec![
            SkillSummary {
                name: "plan".into(),
                description: "Plan A.".into(),
                aliases: vec!["shared".into()],
                source: PathBuf::from("a/SKILL.md"),
            },
            SkillSummary {
                name: "planning".into(),
                description: "Plan B.".into(),
                aliases: vec!["shared".into()],
                source: PathBuf::from("b/SKILL.md"),
            },
        ],
    };

    assert!(catalog.triggered_skill_names("shared").is_empty());
}
```

Add a turn-loop test to `turn.rs`:

```rust
#[tokio::test]
async fn phase_three_injects_summary_and_triggered_skill_instruction() {
    let workspace = test_workspace("phase-three-skill-instructions");
    let skills_root = workspace.join("skills");
    fs::create_dir_all(skills_root.join("planning")).unwrap();
    fs::write(
        skills_root.join("planning").join("SKILL.md"),
        "---\nname: planning\ndescription: Write plans.\n---\n\n# Planning\nUse checklists.",
    )
    .unwrap();
    let catalog = crate::skill_catalog::SkillCatalog::load_development(&skills_root)
        .await
        .unwrap();
    let skills = SkillRegistry::load_development(&skills_root).await.unwrap();
    let config = RuntimeConfig::workspace_write(workspace.clone(), workspace.clone());
    let runner = TurnRunner::new_with_catalog_and_config(
        ScriptedModel {
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            responses: vec![vec![
                GatewayEvent::TextDelta { text: "done".into() },
                GatewayEvent::Completed,
            ]],
        },
        skills,
        catalog,
        config,
    );

    let _events = runner.run("use $planning").await.unwrap();
    let requests = runner.model.requests.lock().unwrap();
    let developer = requests[0].input[1]["content"].as_str().unwrap();

    assert!(developer.contains("<available_skills count=\"1\">"));
    assert!(developer.contains("<skill_instructions name=\"planning\""));
    assert!(developer.contains("Use checklists."));
    remove_workspace(&workspace);
}
```

- [ ] **Step 2: Run trigger tests to verify they fail**

Run:

```bash
pixi run cargo test -p agent-runtime skill_catalog::tests::trigger_policy turn::tests::phase_three_injects_summary_and_triggered_skill_instruction -- --nocapture
```

Expected: tests fail because trigger policy and new constructor are missing.

- [ ] **Step 3: Implement trigger policy and turn wiring**

Implement `SkillCatalog::triggered_skill_names(&self, user_text: &str) -> Vec<String>`.

Rules:

- Exact `$name` matches always trigger.
- Plain text names and aliases trigger only when exactly one skill matches that token.
- Matching is ASCII case-insensitive.
- Return names sorted and deduplicated.

Update `TurnRunner`:

```rust
skill_catalog: crate::skill_catalog::SkillCatalog,
```

Existing constructors use `SkillCatalog::empty()`. `new_with_catalog_and_config` accepts a catalog. `run` builds an `InstructionConfig`, sets `skill_summaries`, computes triggered names, loads full documents with `self.skill_catalog.load_instruction_documents(&triggered_names, self.config.output_limit_bytes)`, and passes them to `InstructionContext`.

If the catalog cannot load a selected full instruction, fail the turn with `TurnFailed` rather than silently dropping the skill.

Do not add skill-root state to `RuntimeConfig`; `SkillCatalog` owns the canonical root that it discovered or loaded from packaged metadata.

- [ ] **Step 4: Wire server startup**

In `crates/agent-server/src/main.rs`, load the catalog from the configured skills root:

```rust
let skill_catalog = SkillCatalog::load_development(&skills_root)
    .await
    .unwrap_or_else(|error| {
        tracing::warn!(?error, "failed to load instruction skill catalog");
        SkillCatalog::empty()
    });
```

Use `TurnRunner::new_with_catalog_and_config`.

- [ ] **Step 5: Run trigger tests to verify they pass**

Run:

```bash
pixi run cargo test -p agent-runtime skill_catalog::tests::trigger_policy turn::tests::phase_three_injects_summary_and_triggered_skill_instruction -- --nocapture
```

Expected: trigger and turn tests pass.

- [ ] **Step 6: Commit trigger task**

Run:

```bash
git add crates/agent-runtime/src/skill_catalog.rs crates/agent-runtime/src/turn.rs crates/agent-runtime/src/tools/mod.rs crates/agent-server/src/main.rs
git commit -m "feat: trigger instruction skills in turns"
```

## Task 5: Compatibility and Packaged Exclusion Coverage

**Files:**
- Modify: `crates/agent-runtime/src/skill_catalog.rs`
- Modify: `crates/agent-runtime/src/skill.rs`
- Modify: `crates/agent-runtime/src/turn.rs`

- [ ] **Step 1: Write compatibility tests**

Add tests proving the Phase 3 acceptance criteria:

```rust
#[tokio::test]
async fn runtime_skill_and_instruction_skill_can_coexist() {
    let root = unique_test_dir("coexist");
    write_skill_md(&root, "coexist", "---\nname: coexist\ndescription: Both forms.\n---\n\n# Coexist").await;
    write_runtime_skill_json(&root, "coexist", "coexist_echo").await;

    let catalog = SkillCatalog::load_development(&root).await.unwrap();
    let registry = crate::skill::SkillRegistry::load_development(&root).await.unwrap();

    assert!(catalog.summaries().iter().any(|summary| summary.name == "coexist"));
    assert!(registry.tools().iter().any(|tool| tool.name == "coexist_echo"));
    remove_test_dir(root).await;
}

#[tokio::test]
async fn runtime_tool_without_skill_md_still_executes() {
    let root = unique_test_dir("runtime-only");
    write_runtime_skill_json(&root, "runtime-only", "runtime_only_echo").await;

    let catalog = SkillCatalog::load_development(&root).await.unwrap();
    let registry = crate::skill::SkillRegistry::load_development(&root).await.unwrap();
    let result = registry
        .execute("runtime_only_echo", serde_json::json!({ "text": "ok" }))
        .await
        .unwrap();

    assert!(catalog.summaries().is_empty());
    assert_eq!(result["text"], "ok");
    remove_test_dir(root).await;
}
```

- [ ] **Step 2: Run compatibility tests**

Run:

```bash
pixi run cargo test -p agent-runtime skill_catalog::tests::runtime_skill_and_instruction_skill_can_coexist skill_catalog::tests::runtime_tool_without_skill_md_still_executes -- --nocapture
```

Expected: tests pass once helper functions are added.

- [ ] **Step 3: Add helper functions and any missing validation**

Add test helpers that write both `SKILL.md` and `skill.json` with `index.js`. Ensure `SkillCatalog::load_development` skips folders without `SKILL.md`, while `SkillRegistry::load_development` skips folders without `skill.json`.

- [ ] **Step 4: Run full Phase 3 focused tests**

Run:

```bash
pixi run cargo test -p agent-runtime skill_catalog::tests -- --nocapture
pixi run cargo test -p agent-runtime instructions::tests::renders_skill_summaries_and_selected_skill_instructions -- --nocapture
pixi run cargo test -p agent-runtime turn::tests::phase_three_injects_summary_and_triggered_skill_instruction -- --nocapture
```

Expected: all focused Phase 3 tests pass.

- [ ] **Step 5: Commit compatibility task**

Run:

```bash
git add crates/agent-runtime/src/skill_catalog.rs crates/agent-runtime/src/skill.rs crates/agent-runtime/src/turn.rs
git commit -m "test: cover instruction skill compatibility"
```

## Task 6: Phase 3 Verification and Documentation

**Files:**
- Modify: `docs/mvp-verification.md`
- Modify: `docs/superpowers/plans/2026-06-28-codex-like-runtime-phase-3.md`

- [ ] **Step 1: Run full verification**

Run:

```bash
pixi run cargo test --workspace
pixi run cargo clippy --workspace --all-targets -- -D warnings
pixi run cargo fmt --all --check
git diff --check HEAD
find crates apps scripts -type f \( -name '*.rs' -o -name '*.ts' -o -name '*.tsx' -o -name '*.js' -o -name '*.jsx' -o -name '*.css' -o -name '*.mjs' \) -not -path '*/target/*' -not -path '*/node_modules/*' -print0 | xargs -0 wc -l | sort -nr | head -20
```

Expected:

- Rust workspace tests pass.
- Clippy has no warnings.
- Rust format check passes.
- Whitespace check exits 0.
- No edited/new source file exceeds 1000 physical lines.

- [ ] **Step 2: Append verification record**

Append a `Codex-Like Runtime Phase 3 Verification` section to `docs/mvp-verification.md` with exact commands and observed results.

Append completion evidence to this plan:

```markdown
## Codex-Like Runtime Phase 3 Completion Evidence

Completed: 2026-06-28

Commits:
- Use `git log --oneline -- docs/superpowers/plans/2026-06-28-codex-like-runtime-phase-3.md crates/agent-runtime/src/skill_catalog.rs crates/agent-runtime/src/instructions.rs crates/agent-runtime/src/turn.rs` and copy the actual Phase 3 commit ids into this section.

Full verification:
- `pixi run cargo test --workspace`: PASS
- `pixi run cargo clippy --workspace --all-targets -- -D warnings`: PASS
- `pixi run cargo fmt --all --check`: PASS
- `git diff --check HEAD`: PASS
- Source line budget: PASS
```

- [ ] **Step 3: Commit documentation**

Run:

```bash
git add docs/mvp-verification.md docs/superpowers/plans/2026-06-28-codex-like-runtime-phase-3.md
git commit -m "docs: record codex-like runtime phase 3 verification"
```

## Phase 3 Acceptance Checklist

- [x] `SkillCatalog` discovers `SKILL.md` instruction skills.
- [x] `SKILL.md` front matter parsing validates `name` and `description`.
- [x] Available skill summaries are injected into model developer context.
- [x] Explicit `$skill-name` mentions load full skill instructions.
- [x] Unique plain-text name or alias mentions load full skill instructions.
- [x] Ambiguous plain-text matches do not inject every full skill.
- [x] Runtime skills and instruction skills can coexist.
- [x] A skill without `skill.json` can still provide instructions.
- [x] A runtime tool without `SKILL.md` can still execute.
- [x] Packaged mode excludes `SKILL.md` unless explicitly configured.
- [x] Production desktop UI remains free of skill/tool management UI.
- [x] `docs/mvp-verification.md` records Phase 3 verification evidence.
- [x] No edited/new source file exceeds 1000 physical lines.

## Codex-Like Runtime Phase 3 Completion Evidence

Completed: 2026-06-28

Commits:
- `18b7e36` docs: add codex-like runtime phase 3 plan
- `75bf257` feat: add skill catalog front matter parser
- `85d2f84` feat: discover instruction skill catalog
- `0ca0a89` feat: inject instruction skill context
- `3c0419e` feat: trigger instruction skills in turns
- `c821992` test: cover instruction skill compatibility

Focused verification:
- `pixi run cargo test -p agent-runtime skill_catalog::tests -- --nocapture`: PASS, 11/11
- `pixi run cargo test -p agent-runtime instructions::tests::renders_skill_summaries_and_selected_skill_instructions -- --nocapture`: PASS
- `pixi run cargo test -p agent-runtime turn::tests::phase_three_injects_summary_and_triggered_skill_instruction -- --nocapture`: PASS
- `pixi run cargo test -p agent-server`: PASS, 11/11

Full verification:
- `pixi run cargo test --workspace`: PASS, `agent-runtime` 123/123, `agent-server` 11/11, `model-gateway` 15/15
- `pixi run cargo clippy --workspace --all-targets -- -D warnings`: PASS
- `pixi run cargo fmt --all --check`: PASS
- `git diff --check HEAD`: PASS
- Source line budget: PASS, no edited/new source file exceeds 1000 physical lines; largest checked source files remain `crates/agent-runtime/src/tools/builtin.rs` and `crates/agent-server/src/api.rs` at 943 lines.
