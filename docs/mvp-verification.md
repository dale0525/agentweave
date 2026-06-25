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

## Known Gaps

- The assistant reply is deterministic and local: `MVP agent received: ...`; it does not call a real model provider.
- There is no real model streaming yet. The server returns normalized runtime events in one JSON response instead of WebSocket or SSE deltas.
- Command skill sandbox limits remain MVP-level and are not equivalent to a hardened Codex sandbox.
- Provider failover is still static/incremental; richer health checks, dynamic routing, and retry policies remain future work.
- The desktop local dev server origins are covered for CORS: `http://127.0.0.1:5173` and `http://localhost:5173`. Additional packaged-app origins may need explicit policy once packaging is implemented.
- The desktop keeps the optimistic user message visible if creating/posting fails; the inline error reports the failure, and a future UI pass can mark or roll back failed optimistic messages.
