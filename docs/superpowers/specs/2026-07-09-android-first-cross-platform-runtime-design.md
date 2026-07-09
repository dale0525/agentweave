# Android-First Cross-Platform Runtime Design

## Summary

GeneralAgent will become a true cross-platform application by making the runtime portable, not by treating mobile as a thin companion to desktop. The first mobile target is Android. Android must be able to run agent turns independently, without connecting to the desktop app or `agent-server`.

The MVP uses a full Rust runtime core embedded in the Android app. Kotlin owns the Android UI and platform integrations, while Rust owns agent behavior, sessions, model calls, skill filtering, tool dispatch, storage, and the file-system sandbox.

The first model path is HTTP provider access. On-device models, push, background long-running tasks, camera, screen, voice, shell, desktop browser automation, and OpenClaw Gateway protocol compatibility are out of scope for the MVP.

## Goals

- Run a complete agent turn on Android without desktop assistance.
- Reuse the same Rust runtime logic across desktop, server, Android, and future iOS.
- Keep platform-specific behavior behind explicit capability registration.
- Let most skills stay platform-independent when they only need model access, HTTP, structured data, or app-private files.
- Disable platform-specific skills when their required capabilities are missing.
- Restrict mobile file operations to the app's private data directory unless an explicit external provider is configured later.
- Store API keys in Android Keystore, not in Rust SQLite or logs.

## Non-Goals

- iOS support in the first implementation phase.
- Running desktop-only tools on mobile.
- Arbitrary mobile file-system access.
- Shell or process execution on Android.
- Headless browser automation on Android.
- Background long-running agent turns.
- Push notifications.
- Camera, screen, voice, canvas, or node invocation features.
- Direct compatibility with OpenClaw Gateway protocol.
- A second Kotlin implementation of the agent runtime.

## Architecture

The system is split into a portable Rust core and thin platform shells.

### Rust Core

Rust owns:

- agent turn loop
- session and message storage
- model-gateway HTTP provider calls
- skill catalog loading
- skill and tool capability filtering
- tool dispatch
- app-data VFS
- runtime event production
- runtime configuration schema
- storage migrations
- diagnostics

Rust must not assume the current process working directory is a writable workspace. File access goes through a platform-provided file provider.

### Android Kotlin Shell

Kotlin owns:

- Jetpack Compose UI
- Android app lifecycle
- Android Keystore access
- app-private directory discovery
- cache directory discovery
- runtime initialization
- platform capability registration
- network and permission status reporting
- native library loading
- conversion of runtime events into UI state

Kotlin does not duplicate agent rules, skill filtering, model calls, or session persistence.

### Mobile FFI Facade

A new Rust crate, `crates/mobile-ffi`, exposes stable high-level APIs to Android. The facade must avoid leaking internal Rust types across the FFI boundary. DTOs should be simple, explicit, serializable, and versionable.

UniFFI is preferred over handwritten JNI if it supports the needed async and callback shape cleanly. If UniFFI becomes awkward for event streaming, use a narrow JNI layer for that part while keeping the facade contract the same.

## Runtime API Surface

The Android shell talks to Rust through high-level runtime operations.

### Initialization

`initialize_runtime(init_config)` starts the core.

Inputs include:

- app data directory
- cache directory
- database path
- bundled skill package path
- platform identifier
- platform capability list
- model config path or database location
- log level

The returned state includes runtime version, active capabilities, database status, skill status, and diagnostics.

### Sessions

Rust exposes:

- `list_sessions()`
- `create_session(title)`
- `get_messages(session_id)`
- `delete_session(session_id)`

Rust owns the underlying SQLite database. Android should not use Room as the source of truth for sessions or messages.

### Turns

Rust exposes:

- `send_message(session_id, content, options) -> turn_id`
- `cancel_turn(turn_id)`
- `subscribe_events(filter) -> event stream`

The Rust core appends the user message, runs the turn, emits runtime events, and persists the assistant message when the turn finishes.

Runtime events must be represented by a stable FFI DTO, derived from the existing `RuntimeEvent` model but not exposing the enum directly if that makes mobile compatibility brittle.

### Skills

Rust exposes:

- `list_skills()`
- `validate_skills()`
- `reload_skills()`

Each skill record includes:

- id
- name
- description
- package path or package id
- availability status
- missing capabilities
- human-readable disabled reason
- whether it contributes tools
- whether it is instruction-only

Unavailable skills are not exposed to the model as usable tools.

### Model Configuration

Rust stores non-secret model configuration:

- provider id
- endpoint type
- base URL
- model name
- optional headers that are not secret
- secret reference id

Android stores API keys in Keystore. Rust obtains the key only when needed, either through a Kotlin callback or a short-lived secret injection API. Rust must not persist the plaintext key.

## Capability Model

Capabilities describe what the current platform host can safely provide. They are registered at runtime initialization.

Initial capability names:

- `network.http`
- `filesystem.app_data`
- `filesystem.external_configured`
- `secure_storage`
- `model.http_provider`
- `browser.headless`
- `shell.process`
- `desktop.automation`
- `camera.capture`
- `media.pick`
- `notifications.local`

Android MVP registers:

- `network.http`
- `filesystem.app_data`
- `secure_storage`
- `model.http_provider`

Android MVP does not register:

- `browser.headless`
- `shell.process`
- `filesystem.unrestricted`
- `desktop.automation`
- `camera.capture`
- `media.pick`
- `notifications.local`

The runtime treats capability checks as enforcement, not just UI hints.

## Skill Manifest

Skills can declare capability requirements in package metadata. A proposed shape:

```json
{
  "id": "web-search",
  "requires": ["network.http", "browser.headless"],
  "optional": ["filesystem.app_data"],
  "platforms": {
    "android": {
      "status": "unsupported",
      "reason": "Requires a desktop headless browser."
    }
  }
}
```

Rules:

- All `requires` capabilities must be present for the skill to be enabled.
- Missing `optional` capabilities may degrade behavior but do not disable the skill.
- `platforms` can provide clearer platform-specific explanations.
- Runtime tools without explicit capability metadata are disabled on Android until they declare their requirements.
- Instruction-only skills may remain available if they do not expose blocked tools.
- Runtime filtering happens before model prompt and tool registry construction.

## File-System Sandbox

Mobile file access uses a VFS layer. Skills receive VFS URIs instead of raw platform paths.

Initial URI spaces:

- `app://documents/...`
- `app://cache/...`
- `app://skills/...`

Future URI spaces:

- `webdav://account/path`
- `provider://android-document-tree/...`
- `sync://workspace/...`

Rules:

- Android MVP allows only app-private documents and cache.
- Absolute host paths are rejected.
- `..` traversal is rejected.
- symlink escapes are rejected.
- external providers require explicit user configuration.
- skill documentation may describe how to enable external providers, but the runtime enforces the configured provider boundary.

This replaces the current desktop-oriented assumption that `RuntimeConfig::workspace_write(cwd, cwd)` is always the right workspace model.

## Android MVP Screens

The Android app needs four initial surfaces:

- Chat: create/select a session, send messages, stream events, show persisted history.
- Model Settings: configure HTTP provider base URL, endpoint type, model, and API key.
- Skills: show available and unavailable skills with missing-capability reasons.
- Diagnostics: show runtime initialization status, storage status, model config status, and skill validation status.

The UI should not describe unavailable skills as errors. They are expected platform capability outcomes.

## Data Flow

### App Startup

1. Kotlin loads the native runtime library.
2. Kotlin reads Android app-private directories.
3. Kotlin checks whether the configured API key exists in Keystore.
4. Kotlin builds the Android capability list.
5. Kotlin calls `initialize_runtime`.
6. Rust opens or migrates SQLite.
7. Rust loads bundled skills.
8. Rust filters skills against Android capabilities.
9. Kotlin renders Chat, Settings, Skills, and Diagnostics from runtime state.

### Model Configuration

1. User enters base URL, endpoint type, model name, and API key.
2. Kotlin stores the API key in Android Keystore.
3. Kotlin sends non-secret model config plus a secret reference to Rust.
4. Rust persists the non-secret config.
5. Diagnostics can validate that a model config exists without exposing the key.

### Sending a Message

1. User submits a message in Compose.
2. Kotlin calls `send_message`.
3. Rust persists the user message.
4. Rust builds the filtered skill/tool context.
5. Rust resolves the API key through the secret bridge.
6. Rust calls the HTTP provider through `model-gateway`.
7. Rust emits runtime events.
8. Kotlin updates the chat UI from events.
9. Rust persists the final assistant message.
10. Kotlin reloads or receives the final message state.

## Security

API keys are stored in Android Keystore. Rust may hold a key only in memory for a model call and must not write it to SQLite, files, or logs.

Capability filtering is enforced by Rust before tools are exposed. A desktop-only skill cannot become available on Android by UI mistake.

The VFS layer prevents mobile skills from escaping app-private storage. Future external storage providers must be explicit opt-ins.

Runtime logs must redact model secrets, headers, file contents, and user credentials.

## Testing

Rust tests:

- capability filtering enables and disables skills correctly
- missing capabilities produce stable reasons
- VFS rejects path traversal and absolute paths
- model config persists without secret values
- runtime can run a fake-model turn through the mobile facade
- sessions and messages persist across runtime restart

Android tests:

- native library loads
- runtime initializes with app-private directories
- API key is stored in Keystore, not Rust storage
- Chat can create a session and render fake-model messages
- Skills screen shows unavailable desktop-only skills
- Settings validates missing and present model config states

Integration tests:

- Android emulator runs one real HTTP model turn when credentials are supplied
- app restart preserves session history
- network failure produces a clear error and does not corrupt history

## Milestones

### 1. Mobile FFI Skeleton

- Add `crates/mobile-ffi`.
- Expose runtime initialization and diagnostics.
- Build and load the native library from Android.

### 2. Rust-Owned Local Sessions

- Move mobile session operations through Rust.
- Use SQLite owned by the Rust core.
- Verify persistence across app restart.

### 3. HTTP Model Turn

- Add model config APIs.
- Add Android Keystore secret bridge.
- Run a real HTTP provider turn from Android.

### 4. Capability-Gated Skills

- Add skill capability metadata support.
- Filter tool registry and instruction context.
- Show skill availability in Android.

### 5. App-Data VFS

- Introduce VFS URI handling.
- Replace mobile workspace assumptions with app-private providers.
- Add file operation tests for sandbox enforcement.

### 6. Compose MVP

- Build Chat, Model Settings, Skills, and Diagnostics screens.
- Wire event streaming to the chat UI.
- Verify the first complete Android-only turn.

## Acceptance Criteria

- Android app completes a real model-backed agent turn without desktop or `agent-server`.
- The app stores and reloads sessions and messages locally.
- API keys are stored in Android Keystore and are absent from Rust SQLite and logs.
- Android lists skills with accurate availability and missing-capability reasons.
- Desktop-only skills are not exposed to the mobile model/tool registry.
- File tools can only access app-data VFS locations.
- Path traversal and absolute path attempts are rejected.
- Turning off network produces a clear error without corrupting history.
- The capability filtering logic is reusable by desktop and server hosts.
- New source files stay focused and below the repository's 1000-line source-file limit.

## OpenClaw Relationship

OpenClaw remains useful as a reference for mobile UX, capability language, pairing, and long-term device features. It should not be treated as the codebase to transplant for the MVP because its mobile clients are companion nodes for an OpenClaw Gateway, while GeneralAgent's Android MVP must host the runtime locally.

The useful ideas to borrow are:

- explicit capabilities
- clear unavailable-feature explanations
- mobile-safe connection and diagnostics patterns
- offline-friendly chat UX

The MVP does not implement the OpenClaw Gateway WebSocket protocol.
