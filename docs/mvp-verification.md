# MVP Verification

Date: 2026-06-25

## Scope

Task 10 verifies the end-to-end MVP chat loop between the desktop Chat Workbench and the local agent server.

The desktop UI reuses the route-specific Stitch sources from Task 9:

- Desktop Chat Workbench: `projects/2766387072810808629/screens/7755a1f98dc24a009945c824739a3834`, DESKTOP 2560x2048, screenshot file `projects/2766387072810808629/files/fd96936f219b4fd697a15d91eec3fe30`
- Mobile Chat Workbench: `projects/2766387072810808629/screens/6eb189a97c6347b9a07aa04b8313132b`, MOBILE 780x1768, screenshot file `projects/2766387072810808629/files/001b14cd4b214041a5c2723a73e453a7`

No new route, major layout, or visual system was introduced for Task 10. The implementation only connects the existing Chat Workbench to the local server and adds the minimal inline error/status states needed for recoverable API failures.

## Automated Checks

| Check | Result | Evidence |
| --- | --- | --- |
| `pixi run test` | PASS | Rust workspace tests passed: `agent-runtime` 8/8, `agent-server` 5/5, `model-gateway` 2/2, doc tests 0 failures. |
| `pixi run cargo clippy --workspace --all-targets -- -D warnings` | PASS | Clippy finished for `model-gateway`, `agent-runtime`, and `agent-server` with no warnings. |
| `cd apps/desktop && pixi run npm test` | PASS | Vitest passed `tests/chat.test.tsx`: 1 file, 5 tests. |
| `cd apps/desktop && pixi run npm exec tsc -- --noEmit` | PASS | TypeScript app check exited 0. |
| `cd apps/desktop && pixi run npm exec tsc -- --noEmit -p tsconfig.vitest.json` | PASS | TypeScript test config check exited 0. |
| `pixi run cargo test -p agent-server supports_vite_desktop_cors_preflight` | PASS | Regression test verifies Vite desktop origin preflight returns CORS headers. The test failed before the CORS fix with HTTP 405. |

## Manual And Smoke Checks

Started the server with the default SQLite storage URL and `RUST_LOG=info`:

- Server log included: `agent server listening on http://127.0.0.1:49321`
- `GET http://127.0.0.1:49321/health` returned `ok`
- `POST /sessions` with JSON returned HTTP 200 for both Vite dev origins
- `POST /sessions/:id/messages` with JSON returned HTTP 200
- Assistant payload returned for `http://127.0.0.1:5173`: `MVP agent received: cors 127 smoke`
- Assistant payload returned for `http://localhost:5173`: `MVP agent received: cors localhost smoke`
- Runtime event types returned: `turn_started,assistant_text_delta,assistant_message_finished,turn_finished`

CORS smoke for the desktop Vite origins:

- `OPTIONS /sessions/session-1/messages` with `Origin: http://127.0.0.1:5173`, `Access-Control-Request-Method: POST`, and `Access-Control-Request-Headers: content-type` returned HTTP 200
- `OPTIONS /sessions/session-1/messages` with `Origin: http://localhost:5173`, `Access-Control-Request-Method: POST`, and `Access-Control-Request-Headers: content-type` returned HTTP 200
- Each preflight response echoed the matching `access-control-allow-origin`
- Preflight responses included `access-control-allow-methods: GET,POST`
- Preflight responses included `access-control-allow-headers: content-type`
- JSON `POST /sessions` and `POST /sessions/:id/messages` with `Origin: http://127.0.0.1:5173` returned `access-control-allow-origin: http://127.0.0.1:5173`
- JSON `POST /sessions` and `POST /sessions/:id/messages` with `Origin: http://localhost:5173` returned `access-control-allow-origin: http://localhost:5173`

## Consumer Chat UI Verification

Date: 2026-06-25

> Repositioning note: The Settings Skills desktop/mobile checks below describe a superseded implementation. The current target is model-only Settings, sourced from Stitch screens `b591242868d74b0093a7f11b2c0c0f8e` and `0a239471a02d413da7880f4ccef955e6`, with no user-facing skill controls.

This pass verifies the consumer chat redesign against the Stitch source of truth:

- Stitch project: `projects/8616130577965446903`
- Stitch design system: `assets/e4d441befa1d42e4af22f64b6d8e5d3c`
- Chat desktop: `projects/8616130577965446903/screens/461eff16a7494012ad9524538fbb0a51`
- Chat mobile: `projects/8616130577965446903/screens/167503d23b82470c8d94be18327febb8`
- Settings Model desktop: `projects/8616130577965446903/screens/649a9bb196474ef59732a3c3f0d1f9d7`
- Settings Model mobile: `projects/8616130577965446903/screens/a57913d00fce464a8c06befeef388c08`
- Settings Skills desktop: `projects/8616130577965446903/screens/7a290ad8d9d5450cafae27b8f5c436fd`
- Settings Skills mobile: `projects/8616130577965446903/screens/86ab488e0e8f47c9bc4ae489798efe96`
- Drawer desktop: `projects/8616130577965446903/screens/acfff82e35404878ba881ddddeb6788e`
- Drawer mobile: `projects/8616130577965446903/screens/e5b849ed880e4170b77d8f6d5ad54284`

Automated checks:

| Check | Result | Evidence |
| --- | --- | --- |
| `pixi run npm --prefix apps/desktop test` | PASS | Vitest passed `tests/chat.test.tsx`: 1 file, 17 tests. |
| `cd apps/desktop && pixi run npm exec tsc -- --noEmit -p tsconfig.vitest.json` | PASS | TypeScript test config check exited 0. |
| `pixi run npm --prefix apps/desktop exec tsc -- --noEmit -p tsconfig.vitest.json` | NOT USED | This exact planned form resolved `tsconfig.vitest.json` from the repo root in this npm environment and failed with TS5058. The equivalent app-cwd command above was used for the TypeScript verification. |
| `git diff --check HEAD && git diff --cached --check` | PASS | Whitespace check exited 0. |
| Contrast check for primary filled controls | PASS | Light fill and hover measured 5.47:1 and 7.58:1; dark fill and hover measured 7.77:1 and 9.78:1. |
| Source line budget | PASS | Renderer TSX/CSS and `apps/desktop/tests/chat.test.tsx` were checked; largest checked file was 388 physical lines. |

Local visual verification:

- Requested Vite command: `pixi run npm --prefix apps/desktop run dev -- --host 127.0.0.1 --port 5173`
- Port `5173` was already occupied by an existing `node` process, so this verification used `http://127.0.0.1:5174/`.
- Actual command: `pixi run npm --prefix apps/desktop run dev -- --host 127.0.0.1 --port 5174`
- Browser console check: 0 warnings and 0 errors.
- Desktop chat checked at `1440x900`.
- Mobile chat checked at `390x844`.
- Desktop conversation drawer checked at `1440x900`.
- Mobile conversation drawer checked at `390x844`.
- Desktop settings Model and Skills tabs checked at `1440x900`.
- Mobile settings Model and Skills tabs checked at `390x844`.
- Dark mode checked for desktop chat at `1440x900` and mobile settings at `390x844`.

Visual review result:

- PASS. The implementation matches the Stitch structure and style direction: chat-first layout, centered message column, temporary conversation drawer, focused settings route, segmented settings tabs, neutral surfaces, teal primary actions, no visible developer workbench panels, no gradients, and no purple decorative treatment.
- Repositioning follow-up required: remove user-facing Skills tab and verify model-only Settings at desktop and mobile breakpoints.
- Layout, spacing, typography scale, hierarchy, color, component states, drawer behavior, and mobile fit were checked against the Stitch screens.
- The mobile settings screenshots were recaptured after resetting tab state; `mobile-settings-model.png` displayed the Model connection form and `mobile-settings-skills.png` displayed the Skills list.
- The mobile composer stayed anchored at the bottom without overlapping the visible starter message. DOM measurement at `390x844` showed the composer at `y=765`, height `79`, and message list bottom padding preserved room for it.
- Dark mode retained the same structure, readable surfaces, and acceptable primary-control contrast.

Known acceptable deviations:

- Stitch reference canvases are `2560x2048` desktop and `780x1768` mobile, while implementation screenshots used runtime browser viewports of `1440x900` and `390x844`.
- Browser font rendering and real Vite content differ slightly from Stitch screenshots.
- The implementation uses larger, looser settings rows and toggle controls than the tighter Stitch references; content hierarchy remains clear and no text is clipped.
- Chat screenshots omit some nonessential Stitch hint/status details and suggested chips; the MVP scope keeps the primary chat, drawer, and settings surfaces simple.
- Dark mode has no direct Stitch screen, so it was verified for structural parity, readability, and contrast rather than pixel parity.

## Developer Framework Repositioning Verification

Date: 2026-06-25

Stitch source of truth:

- Project: `projects/8616130577965446903`
- Design system: `assets/e4d441befa1d42e4af22f64b6d8e5d3c`
- Chat desktop: `d9113e7a1ce640c88135dcd875982cf0`
- Chat mobile: `f74d11de4aa845e0bca5ac976c50352f`
- Settings desktop: `b591242868d74b0093a7f11b2c0c0f8e`
- Settings mobile: `0a239471a02d413da7880f4ccef955e6`

Automated checks:

| Check | Result | Evidence |
| --- | --- | --- |
| `pixi run cargo test --workspace` | PASS | Rust workspace tests passed: `agent-runtime` 13/13, `agent-server` 6/6, `model-gateway` 2/2, doc tests 0 failures. |
| `pixi run cargo clippy --workspace --all-targets -- -D warnings` | PASS | Clippy finished for `agent-runtime` and `agent-server` with no warnings. |
| `pixi run npm --prefix apps/desktop test` | PASS | Vitest passed `tests/chat.test.tsx`: 1 file, 16 tests. |
| `cd apps/desktop && pixi run npm exec tsc -- --noEmit -p tsconfig.vitest.json` | PASS | TypeScript test config check exited 0. |
| `git diff --check HEAD` | PASS | Whitespace check exited 0. |
| Source line budget | PASS | Edited source files were checked; largest edited source file was `crates/agent-runtime/src/skill.rs` at 421 physical lines. |

Local visual verification:

- Actual Vite command: `pixi run npm --prefix apps/desktop run dev -- --host 127.0.0.1 --port 5173`
- Browser console check: 0 warnings and 0 errors.
- Desktop chat checked at `1440x900`.
- Mobile chat checked at `390x844`.
- Desktop settings checked at `1440x900`.
- Mobile settings checked at `390x844`.
- Legacy `/#sessions` hash checked at `1440x900`; it renders the chat UI and does not expose the old Sessions workbench.

Visual review result:

- PASS. The packaged user UI exposes chat and model connection settings only.
- Chat shows the repositioned copy: `Ask naturally. The agent will handle the work.`
- Settings shows only `Model connection`; there are no tabs, switches, skill rows, skill toggles, tool panels, capability chips, diagnostics, or marketplace concepts.
- Programmatic DOM checks found no visible `Skills`, `Active skill`, `use skills`, `skill toggle`, `tool`, `capability`, `diagnostic`, or `marketplace` text in the checked chat/settings routes.
- Mobile chat was corrected to keep the subtitle visible while wrapping within the top bar.

Known acceptable deviations:

- Stitch reference canvases are `2560x2048` desktop and `780x1768` mobile, while runtime checks used browser viewports of `1440x900` and `390x844`.
- Browser font rendering and real Vite content differ slightly from Stitch screenshots.
- The Settings screen does not include extra informational footer content from Stitch; the current implementation intentionally stays within the existing MVP model connection form.

## Codex-Like Runtime Phase 1 Verification

Date: 2026-06-27

Source design:

- `docs/superpowers/specs/2026-06-27-codex-like-runtime-migration-design.md`

Implementation plan:

- `docs/superpowers/plans/2026-06-27-codex-like-runtime-phase-1.md`

Automated checks:

| Check | Result | Evidence |
| --- | --- | --- |
| `pixi run cargo test --workspace` | PASS | Clean archive Rust workspace tests passed: `agent-runtime` 57/57, `agent-server` 11/11, `model-gateway` 15/15, doc tests 0 failures. |
| `pixi run cargo clippy --workspace --all-targets -- -D warnings` | PASS | Clippy completed for `model-gateway`, `agent-runtime`, and `agent-server` with no warnings. |
| `pixi run cargo fmt --all --check` | PASS | Rust formatting check passed. |
| `git diff --check HEAD` | PASS | Whitespace check exited 0. |
| Source line budget | PASS | Edited and created source files were checked in a clean archive; largest checked source file was `crates/agent-server/src/api.rs` at 943 physical lines. |

Runtime behavior verified:

- Built-in `create_directory` can create a workspace directory through the turn loop.
- `read_only` runtime mode blocks write tools with a structured `permission_denied` result.
- Timed-out runtime skill processes are killed before they can continue writing after timeout.
- Runtime skill stdout and stderr are bounded by the configured output limit before JSON parsing.
- Built-in `read_text_file` rejects files larger than the configured output limit before reading them into memory.
- The first model request includes base instructions, tool schemas, and AGENTS.md project instructions.
- Completion endpoints reject non-empty tool schemas with `model_endpoint_does_not_support_tools`.
- Server model-settings turns use the configured runtime workspace root and include workspace instruction context.
- Server message turns return a clear bad request when completion endpoints are selected for tool-using runtime turns.

## Codex-Like Runtime Phase 2 Verification

Date: 2026-06-27

Source design:

- `docs/superpowers/specs/2026-06-27-codex-like-runtime-migration-design.md`

Implementation plan:

- `docs/superpowers/plans/2026-06-27-codex-like-runtime-phase-2.md`

Automated checks:

| Check | Result | Evidence |
| --- | --- | --- |
| `pixi run cargo test --workspace` | PASS | Rust workspace tests passed: `agent-runtime` 110/110, `agent-server` 11/11, `model-gateway` 15/15, doc tests 0 failures. |
| `pixi run cargo clippy --workspace --all-targets -- -D warnings` | PASS | Clippy completed for `agent-runtime` and `agent-server` with no warnings. |
| `pixi run cargo fmt --all --check` | PASS | Rust formatting check passed. |
| `git diff --check HEAD` | PASS | Whitespace check exited 0. |
| Source line budget | PASS | Edited and created source files were checked; largest checked source files were `crates/agent-runtime/src/tools/builtin.rs` and `crates/agent-server/src/api.rs` at 943 physical lines. |

Focused runtime checks:

- `search_files` searches workspace text through `rg` when available and a shell-free fallback when missing.
- `search_files` rejects workspace escapes, caps result count, and truncates long match text safely.
- `exec_command` is hidden by default and requires `workspace_write` plus `command_allowed` mode.
- `exec_command` validates cwd inside the workspace, rejects unknown arguments, applies deny rules, enforces command timeouts, terminates Unix process groups where supported, and returns bounded stdout/stderr metadata.
- `apply_patch` adds, updates, and deletes workspace files through the turn loop.
- `apply_patch` rejects outside-workspace paths, symlink parent escapes, malformed patches, no-anchor hunks, ambiguous duplicate context, and conflicting multi-operation patches.
- Turn-loop tests prove `search_files`, `exec_command`, and `apply_patch` can execute through model tool calls.

Notes:

- `exec_command` remains development-only and non-interactive in Phase 2.
- `apply_patch` intentionally implements a minimal text patch grammar. Moves, binary patches, and full no-final-newline fidelity remain out of scope.
- Approval workflows, stronger sandbox profiles, network policy, MCP, connectors, and Codex-style skill catalog behavior remain later phases.

## Known Gaps

- The assistant reply is deterministic and local: `MVP agent received: ...`; it does not call a real model provider.
- There is no real model streaming yet. The server returns normalized runtime events in one JSON response instead of WebSocket or SSE deltas.
- Command skill sandbox limits remain MVP-level and are not equivalent to a hardened Codex sandbox.
- Provider failover is still static/incremental; richer health checks, dynamic routing, and retry policies remain future work.
- The desktop local dev server origins are covered for CORS: `http://127.0.0.1:5173` and `http://localhost:5173`. Additional packaged-app origins may need explicit policy once packaging is implemented.
- The desktop keeps the optimistic user message visible if creating/posting fails; the inline error reports the failure, and a future UI pass can mark or roll back failed optimistic messages.
