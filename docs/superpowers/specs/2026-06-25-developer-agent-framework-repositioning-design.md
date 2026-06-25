# Developer Agent Framework Repositioning Design

Date: 2026-06-25

## Goal

Reposition GeneralAgent as a developer-facing agent application framework.
Developers can add skills freely while building an application. After packaging, skills are invisible to end users and are used automatically by the agent during conversation.

This design replaces the previous direction where the desktop app exposed a user-facing skill management surface. Skills remain a core runtime mechanism, but they are no longer a product concept that end users configure.

## Product Positioning

GeneralAgent is for developers who want to build packaged agent applications with custom capabilities.

The end user experience is:

- Open the app.
- Configure only user-appropriate settings, such as the model connection if the app chooses to expose it.
- Chat naturally.
- Let the agent decide when to use built-in capabilities.

The developer experience is:

- Add app-specific skills during development.
- Test skills in a development runtime.
- Package a fixed skill manifest into the app.
- Ship an application where skills are internal agent capabilities rather than visible user features.

## Key Decision

Use packaged hidden skills as the default model.

Development-time skills can change freely. Packaged applications ship with a frozen, validated skill bundle. End users cannot add, remove, toggle, inspect, or choose skills through the product UI.

This keeps the product boundary clear:

- Developers own capability composition.
- Runtime owns skill discovery, injection, and execution.
- Users own intent expressed through conversation.

## Personas

### Framework Developer

Builds GeneralAgent itself and maintains runtime, packaging, API, and desktop surfaces.

Needs:

- Stable skill bundle conventions.
- Clear dev-mode and packaged-mode behavior.
- Tests for runtime loading, packaging validation, and hidden skill execution.

### App Developer

Builds a specific agent application using GeneralAgent.

Needs:

- A simple way to create skill packages.
- Fast local testing.
- Packaging checks that catch invalid manifests or missing resources.
- Confidence that internal skills will not leak into the user UI.

### End User

Uses the packaged app.

Needs:

- A simple chat experience.
- Plain-language errors.
- No need to understand tools, skills, manifests, or routing.

## Developer Skill Authoring

Developers should be able to use Codex's built-in `skill-creator` skill during development to create and maintain skill packages.

The intended flow is:

1. The app developer asks Codex to use `skill-creator` to create a new skill.
2. `skill-creator` creates a Codex-style skill folder with `SKILL.md` and optional `scripts/`, `references/`, and `assets/`.
3. The developer adds or generates the GeneralAgent runtime manifest for that package.
4. GeneralAgent dev tooling validates the runtime manifest and executable resources.
5. Packaging includes only the approved runtime files and resources needed by the app.

Codex skills and GeneralAgent runtime skills are related but not identical:

- Codex `SKILL.md` describes how Codex should work during development.
- GeneralAgent `skill.json` describes runtime tools that the packaged app can expose to the model.
- Some packages may contain both files.
- `SKILL.md` is a development asset by default and should not be surfaced to end users.

This distinction lets developers benefit from Codex's skill authoring workflow without turning Codex's internal skill format into the public user-facing product model.

## Skill Package Shape

For the current runtime, keep the existing command-based manifest shape:

```text
skills/
  summarize-repo/
    SKILL.md
    skill.json
    index.js
    references/
    scripts/
```

`skill.json` remains the runtime contract:

```json
{
  "name": "summarize-repo",
  "description": "Summarize repository structure and important files.",
  "version": "0.1.0",
  "entry": {
    "type": "command",
    "command": "node",
    "args": ["index.js"]
  },
  "tools": [
    {
      "name": "summarize_repo",
      "description": "Summarize a repository path.",
      "input_schema": {
        "type": "object",
        "properties": {
          "path": { "type": "string" }
        },
        "required": ["path"]
      }
    }
  ]
}
```

Future tooling can generate `skill.json` from a developer prompt or from structured metadata, but the packaged runtime should continue to rely on an explicit machine-readable manifest.

## Modes

### Development Mode

Development mode can:

- Load skills from a local `skills/` directory.
- Support hot reload or restart-based reload.
- Expose developer diagnostics.
- Show loaded skill names in logs or developer-only panels.
- Support validation commands for skill manifests and executables.
- Let Codex use `skill-creator` to scaffold or update skill packages.

Development diagnostics must be clearly marked as developer-only.

### Packaged Mode

Packaged mode must:

- Load skills from the packaged bundle or generated manifest.
- Treat skill inventory as immutable.
- Hide skill inventory from end-user UI.
- Avoid user-facing skill toggles.
- Avoid public user APIs for listing or enabling skills.
- Report failures as app-level assistant errors, not as raw skill internals.

Packaged mode may log internal skill failures for developers, but user-facing text should stay plain and non-technical.

## Runtime Design

`SkillRegistry` remains a core runtime component, but its meaning changes from "user-manageable skills" to "application capability registry."

Responsibilities:

- Load validated packaged skills.
- Expose tool schemas to the agent turn loop.
- Execute tool calls.
- Enforce timeouts and resource limits.
- Return structured JSON tool results.
- Produce internal runtime events for debugging and storage.

The agent turn loop should continue to:

1. Build the model input from user message, session context, system instructions, and packaged tool schemas.
2. Stream or receive model output.
3. Execute tool calls through `SkillRegistry`.
4. Send tool results back into the model loop.
5. Finish with user-facing assistant text.

The user should never need to know which tool was called unless the app developer intentionally designs a domain-specific explanation into the assistant response.

## API Design

Production APIs should avoid user-facing skill management.

Keep or build:

- `POST /sessions`
- `POST /sessions/:id/messages`
- Session history APIs.
- Model profile APIs if the packaged app exposes provider configuration.
- Internal runtime event storage.

Remove or defer from production:

- `GET /skills`
- Skill enable/disable APIs.
- Skill marketplace APIs.
- Any endpoint that frames skills as user-selectable app features.

Allow in development only:

- `GET /dev/skills`
- `POST /dev/skills/reload`
- `POST /dev/skills/validate`

Development APIs must be disabled by default in packaged builds.

## Desktop UI Design

The desktop app should not expose a Skills tab or skill toggles to end users.

Update the existing UI direction:

- Remove Settings > Skills.
- Keep Settings focused on Model or other user-appropriate app configuration.
- Remove chat copy such as "use skills when you need them."
- Replace it with natural wording, for example: "Ask naturally. The agent will handle the work."
- Avoid tool-call panels, skill status chips, and capability lists in the primary chat UI.

Developer-only diagnostics, if added later, should live behind an explicit dev mode and should not appear in the normal packaged experience.

## Packaging Design

Packaging should freeze the skill inventory.

Expected packaging checks:

- Discover runtime skill manifests.
- Validate manifest schema.
- Validate entry commands and referenced files.
- Validate tool names and JSON schemas.
- Build a packaged skill index.
- Fail packaging if a skill is invalid.
- Exclude development-only files from production packages.

`SKILL.md` files created by `skill-creator` are development assets by default. Production packages should exclude them. Internal debugging packages may include them only when a developer explicitly enables that packaging option.

## Migration From Current MVP Direction

The current repo contains a consumer chat redesign that includes a user-facing Skills settings tab. That direction should be revised.

Required changes for implementation planning:

- Update feasibility and roadmap docs to describe GeneralAgent as a developer framework.
- Remove "Skill management screen" from end-user MVP milestones.
- Replace user-facing skill settings with developer-mode skill validation and diagnostics.
- Remove fixture skill rows from the desktop settings UI.
- Update tests that assert skill tab behavior.
- Keep runtime `SkillRegistry` tests, but reframe them around packaged application capabilities.

## Testing Strategy

Automated tests should cover:

- Loading a valid runtime skill package.
- Rejecting invalid manifests.
- Executing a packaged command skill.
- Injecting tool schemas into the turn loop.
- Completing a tool call turn without exposing skill state in user-facing responses.
- Ensuring production settings UI has no Skills tab.
- Ensuring dev-only skill APIs are absent or disabled in packaged mode.

Manual checks should cover:

- Developer creates a skill package with Codex `skill-creator`.
- Developer adds a GeneralAgent `skill.json`.
- Dev runtime loads and executes the skill.
- Packaging freezes the skill inventory.
- Packaged UI shows chat and model settings only.

## Non-Goals

- No end-user skill marketplace.
- No end-user skill toggles.
- No packaged-mode dynamic skill installation.
- No guarantee that every Codex `SKILL.md` can become a runtime tool automatically.
- No full plugin permission system in this repositioning pass.

## Open Follow-Ups

- Decide whether GeneralAgent should provide a helper command that converts a `skill-creator` scaffold into a runtime `skill.json`.
- Define the exact packaged skill index file format.
- Decide whether packaged apps can choose to hide model settings as well.
- Define how developer diagnostics are enabled and authenticated.
- Define a future permission model for skills that need filesystem, network, or app-specific resources.
