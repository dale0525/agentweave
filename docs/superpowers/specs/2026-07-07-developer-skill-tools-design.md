# Developer Skill Tools Design

Date: 2026-07-07

## Goal

Add development-only tools for managing skill packages that can be bundled into a final GeneralAgent app.

The tools help app developers inspect, validate, create, update, and delete local skill packages during development. They do not expose skill management to end users, and they do not turn packaged applications into dynamic skill marketplaces.

## Product Decision

Build a development workbench that manages local `skills/` packages and guides the developer back to Codex's existing `skill-creator` workflow for interactive skill authoring.

GeneralAgent remains responsible for:

- Discovering local skill package status.
- Validating runtime manifests and instruction skill metadata.
- Deleting local skill package folders safely.
- Reloading development diagnostics.
- Generating clear Codex prompts for creating or modifying skills with `skill-creator`.

Codex remains responsible for:

- Interactively creating and revising Codex-style `SKILL.md` authoring assets.
- Helping the developer design skill instructions, references, scripts, and runtime manifests.
- Making nuanced file edits in the current repository.

This avoids duplicating `skill-creator` inside the desktop app while still making skill package management visible and practical during development.

## Scope

In scope:

- A developer-only desktop route, `#developer`.
- A skill package list for the configured local `skills/` root.
- Per-package status for instruction assets, runtime manifests, runtime tools, validation state, and bundle readiness.
- Actions to refresh, validate, delete, and generate Codex `skill-creator` prompts.
- Development-only HTTP APIs under `/dev/skills`.
- Automated tests for backend safety and frontend behavior.

Out of scope:

- End-user skill settings, toggles, installation, marketplaces, or runtime selection.
- A full in-app skill generation wizard.
- Automatic conversion of arbitrary `SKILL.md` files into runtime tools.
- Packaged-mode skill mutation.
- A permission model beyond existing runtime validation and safe filesystem boundaries.

## Existing Context

The repository already separates runtime skills and instruction skills:

- `SkillRegistry` loads executable runtime tools from `skill.json`.
- `SkillCatalog` loads Codex-style instruction skills from `SKILL.md`.
- Development mode discovers local packages directly from the `skills/` directory.
- Packaged mode reads a frozen `skill-bundle.json`.
- `crates/agent-server/src/dev_api.rs` already exposes development-only tool diagnostics and instruction preview endpoints.
- Production routing only mounts development APIs when `GENERAL_AGENT_DEV_API=1`.

The current `skills/echo` package is runtime-only: it has `skill.json` and `index.js`, but no `SKILL.md`. The new workbench must represent runtime-only, instruction-only, and combined packages without implying that all packages need both files.

## User Experience

### Entry Point

Add a developer route at `#developer`. This route is not part of the end-user Settings experience.

When the dev API is available, Settings should show a development-only tool button that opens `#developer`. When the dev API is unavailable, Settings should not show the developer entry point. Direct navigation to `#developer` should still show a clear development-disabled state rather than failing silently.

### Desktop Layout

The developer workbench uses a dense tool layout:

- Header with a back action, page title, dev API status, refresh, and validate-all actions.
- Left or primary list of skill packages.
- Right or detail panel for the selected package.
- Compact status chips for package type:
  - Runtime tool package.
  - Instruction package.
  - Combined package.
  - Invalid package.
  - Bundle-ready package.
- Destructive delete action separated from normal actions.

### Mobile Layout

The mobile route uses a single-column version:

- Header actions collapse into icon buttons.
- Package list appears first.
- Selecting a package opens details below the selected row or in a focused detail panel.
- Delete remains behind an explicit confirmation.

### Skill-Creator Prompts

The workbench does not create skills itself. Instead it provides copyable prompts for Codex:

- New skill prompt: asks Codex to use `skill-creator` to create a skill under the configured `skills/` root and to add or update the GeneralAgent `skill.json` runtime manifest when runtime tools are needed.
- Modify skill prompt: includes the selected package path, current files present, runtime tool names, and validation errors, then asks Codex to use `skill-creator` to revise the package.

The prompt should be explicit that `SKILL.md` is a development authoring asset and `skill.json` is the packaged runtime contract.

## API Design

Development routes are mounted only through `router_with_dev_routes`.

### `GET /dev/skills`

Returns the current local skill package inventory.

Response shape:

```json
{
  "root": "/absolute/path/to/skills",
  "packages": [
    {
      "id": "echo",
      "path": "echo",
      "name": "echo",
      "description": "Echo a text payload.",
      "hasSkillMd": false,
      "hasRuntimeManifest": true,
      "runtimeTools": ["echo"],
      "packageKind": "runtime",
      "bundleReady": true,
      "validation": {
        "ok": true,
        "errors": [],
        "warnings": []
      }
    }
  ]
}
```

`packageKind` values:

- `runtime`
- `instruction`
- `combined`
- `empty`
- `invalid`

### `POST /dev/skills/validate`

Validates all package folders and returns the same inventory shape with validation results refreshed.

The endpoint should validate:

- `skill.json` parseability and existing `SkillRegistry` manifest rules.
- `SKILL.md` front matter parseability and existing `SkillCatalog` rules.
- Duplicate runtime tool names across packages.
- Duplicate instruction skill names across packages.
- Entry resource files referenced by runtime manifests.
- Safe single-directory package shape.

### `POST /dev/skills/reload`

Reloads development skill diagnostics and returns the current inventory.

For this pass, reload means a fresh filesystem scan for diagnostics. It does not replace the already-created live `SkillRegistry` or `SkillCatalog` inside active agent runners.

### `DELETE /dev/skills/{id}`

Deletes one local skill package directory.

Safety rules:

- `id` must be a single safe path segment.
- Absolute paths, `..`, separators, and empty ids are rejected.
- The target must canonicalize under the configured skills root.
- Symlink escapes are rejected.
- Only package directories are deleted.
- The endpoint returns the refreshed inventory after deletion.

## Backend Design

Add a development skill inventory module or focused helpers in `dev_api.rs`.

Responsibilities:

- Resolve the skills root from an explicit `AppState` field.
- Scan immediate child directories under the skills root.
- Read `skill.json` when present and reuse `SkillRegistry` validation behavior where possible.
- Read `SKILL.md` when present and reuse `SkillCatalog` parsing behavior where possible.
- Preserve partial results when one package is invalid.
- Return structured validation errors instead of failing the whole list when possible.

The current `AppState` stores loaded skills and catalog, but not the source skills root. Add `skills_root: Option<PathBuf>` to support dev inventory APIs without guessing from process cwd.

Production route construction must remain unchanged: development routes are still absent unless the dev router is explicitly mounted.

## Frontend Design

Add a `DeveloperTools` screen and route state for `#developer`.

Frontend API helpers:

- `listDevSkills()`
- `validateDevSkills()`
- `reloadDevSkills()`
- `deleteDevSkill(id)`

Screen components:

- `DeveloperTools`
- `SkillPackageList`
- `SkillPackageDetail`
- `SkillValidationSummary`
- `SkillCreatorPromptDialog`
- `DeleteSkillDialog`

Use existing React and lucide icons. Use Radix Dialog for prompt and delete confirmation because the desktop app already depends on `@radix-ui/react-dialog`.

The implementation should follow Stitch-generated desktop and mobile screens before coding. The Stitch project should reuse an existing GeneralAgent project where possible, and screen IDs must be recorded in the implementation notes.

## Data Flow

1. User opens `#developer`.
2. Frontend calls `GET /dev/skills`.
3. If the endpoint is unavailable, the page shows a dev-disabled state.
4. If available, the page renders package inventory and selects the first package by default.
5. Refresh and reload actions call dev endpoints and replace local inventory state.
6. Validate calls `POST /dev/skills/validate`.
7. Delete opens confirmation, calls `DELETE /dev/skills/{id}`, and updates the list.
8. Create or modify opens a prompt dialog that the developer can use in Codex.

## Error Handling

Backend errors:

- Invalid package ids return `400`.
- Unknown package ids return `404`.
- Unsafe paths return `400`.
- Filesystem failures return `500` and are logged server-side.
- Per-package validation failures are returned as structured validation entries, not top-level HTTP failures.

Frontend errors:

- Dev API unavailable: show a disabled development state.
- Validation errors: show inline diagnostics on package rows and details.
- Delete failures: keep the package in the list and show an error message.
- Empty skills root: show an empty state with the create prompt action.

## Packaging Behavior

The feature must not change packaged-mode loading rules.

Packaged apps continue to:

- Load runtime skills from `skill-bundle.json`.
- Treat the skill inventory as immutable.
- Hide skill management from end-user surfaces.
- Include instruction skills only when package metadata explicitly opts in.

The developer workbench may help identify bundle-ready packages, but it does not package the app in this pass.

## Testing Strategy

Backend tests:

- Development skill routes are not mounted by default.
- `GET /dev/skills` lists runtime-only, instruction-only, combined, empty, and invalid packages.
- Validation reports invalid `skill.json` without hiding other packages.
- Validation reports invalid `SKILL.md` front matter.
- Duplicate runtime tool names are reported.
- Duplicate instruction skill names are reported.
- Delete rejects unsafe ids and symlink escapes.
- Delete removes only the selected package and returns refreshed inventory.

Frontend tests:

- Developer route renders dev-disabled state when `/dev/skills` returns 404 or network failure.
- Developer route renders package list and selected details.
- Runtime-only packages do not display as broken solely because `SKILL.md` is absent.
- Prompt dialog generates a new-skill `skill-creator` prompt.
- Prompt dialog generates a modify-skill `skill-creator` prompt with validation errors.
- Delete confirmation calls the delete endpoint and updates inventory.

Manual checks:

- Run the app with `GENERAL_AGENT_DEV_API=1`.
- Open `#developer` on desktop and mobile viewport sizes.
- Validate the existing `skills/echo` package.
- Generate a create prompt and use Codex `skill-creator` to scaffold a package.
- Refresh and validate the new package.
- Delete a test package and confirm it is removed from disk.
- Run packaged or normal server mode and verify dev routes are unavailable.

## Acceptance Criteria

- Developers can see all local skill packages in development mode.
- Developers can validate whether packages are ready for runtime bundling.
- Developers can safely delete local package folders.
- Developers can generate clear Codex prompts for `skill-creator` creation and modification flows.
- End users still do not see skill management in normal settings or chat.
- Production routes still do not expose development skill APIs.
- Automated tests cover the new backend and frontend behavior.

## Follow-Ups

- Add package index generation for `skill-bundle.json`.
- Add a packaging preview that shows exactly which files will ship.
- Add runtime hot reload for active agent state if product needs justify the extra state management.
- Add a richer permission and resource manifest for skills.
