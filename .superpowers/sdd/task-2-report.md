# Task 2 Report: Development Skill HTTP API

## Status

Completed.

## Scope Delivered

Implemented Task 2 exactly within the owned server API surface:

- `crates/agent-server/src/api.rs`
  - Added `AppState::skills_root: Option<PathBuf>`.
  - Initialized `skills_root` to `None` in existing constructors.
  - Added `AppState::with_skills_root(skills_root: PathBuf) -> Self`.
  - Added `AppState::skills_root(&self) -> Option<PathBuf>`.
  - Expanded desktop CORS methods to include `DELETE`.
- `crates/agent-server/src/dev_api.rs`
  - Mounted:
    - `GET /dev/skills`
    - `POST /dev/skills/validate`
    - `POST /dev/skills/reload`
    - `DELETE /dev/skills/{skill_id}`
  - Implemented handlers using:
    - `crate::dev_skills::scan_skill_packages`
    - `crate::dev_skills::delete_skill_package`
  - Added focused delete error-to-status mapping helper for required endpoint behavior.
  - Added Task 2 API tests for:
    - route absent by default
    - inventory returned when dev routes are enabled
    - unsafe delete id rejected with `400`
    - package deletion returning updated inventory
- `crates/agent-server/src/main.rs`
  - Wired `skills_root` into app state when building the server.

## TDD Record

### Red

1. Added Task 2 API tests first in `crates/agent-server/src/dev_api.rs`.
2. Ran:

```bash
pixi run cargo test -p agent-server dev_ -- --nocapture
```

Observed expected failures for missing:

- `AppState::skills_root()`
- `AppState::with_skills_root(...)`
- dev skill routes depending on those APIs

3. After the first implementation pass, re-ran the same command and got one remaining failing test:

- `dev_delete_skill_rejects_unsafe_id`

The endpoint returned `500` instead of the required `400`, which confirmed the delete-status mapping still needed adjustment.

### Green

Implemented the smallest production changes needed to satisfy the new tests:

- added `skills_root` state plumbing;
- mounted the four dev skill routes;
- delegated list/validate/reload to `scan_skill_packages(...)`;
- delegated delete to `delete_skill_package(...)`;
- tightened delete error mapping to classify unsafe or invalid ids as `400`.

Re-ran the same focused command until the full `dev_` suite passed.

## Verification Run

Final fresh verification command:

```bash
pixi run cargo test -p agent-server dev_ -- --nocapture
```

Result:

- `agent-server`: 13 passed, 0 failed

## Git Hygiene

- Verified `crates/agent-server/src/api.rs` had unrelated pre-existing unstaged changes before staging.
- Staged `crates/agent-server/src/dev_api.rs` and `crates/agent-server/src/main.rs` directly.
- Staged only Task 2 hunks from `crates/agent-server/src/api.rs` with patch mode.
- Confirmed staged file list before commit:
  - `crates/agent-server/src/api.rs`
  - `crates/agent-server/src/dev_api.rs`
  - `crates/agent-server/src/main.rs`

## Commit

- `53cc416 feat: expose development skill routes`

## Notes

- I intentionally did not revert or overwrite unrelated uncommitted changes elsewhere in the repo.
- The unrelated `api.rs` hunks remained unstaged and were not included in the Task 2 commit.
- The report file itself was written after the task code commit so the code commit stayed limited to the three owned source files.

---

## Post-review Fix: Dev Route Desktop CORS Coverage

### Review Finding Addressed

- Important: dev routes merged by `router_with_dev_routes(...)` did not inherit the desktop CORS layer that had already been applied inside `router(...)`, so browser preflight for `DELETE /dev/skills/{id}` could fail.
- Minor: add a focused regression test for the dev delete preflight.

### Root Cause

`router(state)` applied `.layer(desktop_cors_layer())` before `router_with_dev_routes(...)` merged in `crate::dev_api::router(state)`.

In Axum, layering one router does not retroactively wrap routes merged later from another router, so the dev-side `DELETE /dev/skills/{id}` route handled `OPTIONS` without the desktop CORS middleware and returned `405`.

### TDD Record

1. Added a regression test in `crates/agent-server/src/dev_api.rs` that sends:
   - `OPTIONS /dev/skills/echo`
   - `Origin: http://127.0.0.1:5173`
   - `Access-Control-Request-Method: DELETE`
   - `Access-Control-Request-Headers: content-type`
2. Ran:

```bash
pixi run cargo test -p agent-server dev_delete_skill_supports_desktop_cors_preflight -- --nocapture
```

Observed the expected failure:

- response status was `405` instead of `200`

3. Applied the smallest production fix and re-ran the same test until it passed.

### Code Change

In `crates/agent-server/src/api.rs`:

- changed `router_with_dev_routes(...)` to merge `crate::dev_api::router(state).layer(desktop_cors_layer())`
- widened `desktop_cors_layer()` visibility to `pub(crate)` so it remains reusable from the dev-router composition point

This keeps production behavior unchanged:

- `router(...)` still excludes dev routes
- dev routes are still present only through `router_with_dev_routes(...)`
- the same desktop CORS policy now applies to both the base routes and the dev routes

### Regression Coverage

Added focused coverage in `crates/agent-server/src/dev_api.rs` asserting:

- preflight succeeds for `/dev/skills/echo`
- `Access-Control-Allow-Origin` echoes `http://127.0.0.1:5173`
- `Access-Control-Allow-Methods` includes `DELETE`
- `Access-Control-Allow-Headers` includes `content-type`

### Verification

Fresh full verification command:

```bash
pixi run cargo test -p agent-server dev_ -- --nocapture
```

Result:

- passed
