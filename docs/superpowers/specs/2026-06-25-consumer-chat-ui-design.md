# Consumer Chat UI Design

Date: 2026-06-25

> Superseded note: The user-facing Skills settings direction in this document has been replaced by `docs/superpowers/specs/2026-06-25-developer-agent-framework-repositioning-design.md`. GeneralAgent is now positioned as a developer-facing agent framework. Packaged apps hide skills from end users and use them automatically at runtime.

## Goal

Redesign the desktop client from a developer-oriented app-agent workbench into a consumer-friendly chat application. The MVP should feel approachable to ordinary users while preserving the existing server-backed chat loop, multi-session direction, model configuration direction, and pluggable skill direction.

This design intentionally removes the visible developer console feel from the primary experience. Runtime logs, inspector panels, token counters, endpoint chips, and technical tool-call surfaces should not appear on the main chat screen.

## Confirmed Scope

- Build a chat-first interface.
- Do not include a preview screen in this MVP.
- Keep conversation history available through a hidden drawer, not a permanent sidebar.
- Add a settings surface with two sections: model connection and skill management.
- Follow the system color scheme by default with light and dark themes.
- Keep the existing MVP chat API and deterministic local assistant reply behavior.
- Do not implement persistent provider or skill configuration in this UI pass unless it is very low-risk and clearly scoped.

## Product Shape

The app has three user-facing areas.

### Chat

The chat screen is the default route and the main product surface.

It contains:

- A compact top bar.
- A conversation-history button on the left.
- The product/session title in the center or left-center.
- A settings button on the right.
- A centered message stream.
- A bottom composer with a text input and send button.

The chat screen should not show:

- Runtime inspector panels.
- Logs.
- Token usage.
- Raw tool-call panels.
- Provider endpoint chips.
- Always-visible skill lists.
- Technical status labels such as `chat/completions`.

The user flow is:

1. User opens the app and immediately sees the chat.
2. User types a message and sends it.
3. The user message appears in the message stream.
4. The assistant response appears when the server returns.
5. If the request fails, a plain-language inline error appears above the composer.

Error copy should be user-facing and short, for example:

> Could not send message. Check your model or service connection.

### Conversation Drawer

Conversation history is available but not always visible.

The drawer opens from the top-left conversation button. It contains:

- New chat action.
- Search field.
- Recent conversation list.
- Compact metadata such as updated time.

The drawer should feel like a temporary navigation layer. On desktop it can slide over the left side of the chat. On mobile it should cover most of the screen or become a full-height sheet.

The MVP can use mock conversation data if the server does not yet expose `GET /sessions`.

### Settings

Settings is reached from the top-right settings button. It can be a route, modal, or full-height panel as long as it is easy to leave and does not clutter the chat.

Settings has two tabs or segmented sections.

#### Model

Fields:

- Endpoint type: Responses, Chat Completions, or Completion.
- Base URL.
- API key.
- Model name.
- Test connection action.

The MVP setting form can be front-end only if persistence is not implemented in this pass. Empty or mock values are acceptable, but the UI should make the intended configuration model clear.

#### Skills

Fields and controls:

- Skill list.
- Enable/disable toggle per skill.
- Skill name and short description.
- Status indicator such as enabled, disabled, or unavailable.

The MVP can render known local skills from fixture/static data if no API exists yet. The surface should be ready for later connection to the skill registry.

## Visual Direction

Use a restrained consumer chat aesthetic, not a technical workbench.

### Theme

The app should follow `prefers-color-scheme` by default.

Light theme:

- Soft white and light gray surfaces.
- Subtle neutral borders.
- High contrast text.
- Restrained teal/green accent for primary actions.

Dark theme:

- Neutral dark gray surfaces.
- Gentle borders and reduced contrast compared with the current industrial style.
- The same accent family as light mode.

Avoid:

- Developer-console styling.
- Dense three-pane layouts.
- Large dashboard cards.
- Gradients, bokeh, or decorative blobs.
- Industrial copper/cyan status-heavy palette in the main UI.

### Layout

Desktop:

- Single main chat column with comfortable max width.
- Top bar spans the window.
- Drawer overlays or slides from the left.
- Settings opens as a focused panel or route.

Mobile:

- Single column.
- Top bar remains compact.
- Conversation drawer becomes a sheet.
- Composer stays anchored at the bottom.
- Text and buttons must fit without overlap.

### Components

Use familiar primitives:

- Icon buttons for menu, settings, new chat, close, and send.
- Text inputs for composer and settings fields.
- Segmented controls or tabs for settings sections.
- Toggles for skill enablement.
- Clear primary/secondary button hierarchy.

Rounded corners should stay at 8px or less. Cards should be used only for repeated items or settings groups, not as nested decorative containers.

## Architecture

Keep the existing React/Vite/Electron skeleton.

Suggested renderer structure:

- `App.tsx`: route/view state and top-level layout.
- `screens/Chat.tsx`: chat surface and conversation drawer wiring.
- `screens/Settings.tsx`: settings screen/panel.
- `components/ConversationDrawer.tsx`: hidden conversation history drawer.
- `components/Composer.tsx`: message input.
- `components/MessageList.tsx`: message rendering.
- `components/SettingsModel.tsx`: model form section.
- `components/SettingsSkills.tsx`: skill management section.
- `api.ts`: existing server chat API helpers.

The implementation should split files before any source file approaches 1000 physical lines.

## Data Flow

Chat send flow remains:

1. Chat component captures the user input.
2. If no server session exists, create one through `POST /sessions`.
3. Send the message through `POST /sessions/:id/messages`.
4. Extract assistant text from normalized events or assistant message payload.
5. Render the assistant response.
6. Show a short error message if any step fails.

Conversation drawer data can be static for this UI pass.

Settings data can be static/local component state for this UI pass.

## Testing

Add or update renderer tests for:

- Sending a message still creates/posts through the existing API helpers and displays the assistant response.
- Opening and closing the conversation drawer.
- Navigating to settings and back to chat.
- Switching settings tabs between Model and Skills.
- Skill toggle state changes visually and semantically.
- Empty chat messages are still ignored.
- API failure still renders a plain-language error.

Manual/browser verification should cover:

- Chat desktop viewport.
- Chat mobile viewport.
- Conversation drawer desktop and mobile behavior.
- Settings desktop and mobile behavior.
- Light and dark theme behavior if feasible in local browser checks.

## Stitch Requirements

Before implementation, generate or reuse concrete Stitch screens for:

- Desktop chat screen.
- Mobile chat screen.
- Desktop settings screen.
- Mobile settings screen.
- Conversation drawer state if Stitch generation supports state-specific screens.

The current developer workbench screens are not the implementation target for this redesign. They can be used only as historical reference for what to simplify away.

Implementation must be visually compared against the new Stitch screens at matching desktop and mobile viewport sizes.

## Non-Goals

- No preview screen in this MVP.
- No real model-provider persistence unless it is explicitly added to a later implementation plan.
- No live skill marketplace or plugin install workflow.
- No raw runtime event inspector in the main chat.
- No WebSocket/SSE streaming requirement in this UI pass.
- No full Electron packaging work.

## Open Follow-Ups

- Whether settings should become a route, modal, or side panel can be decided during Stitch exploration.
- Whether conversation history should later connect to a `GET /sessions` endpoint is a follow-up backend task.
- Whether failed optimistic messages should become retryable message rows is a later UX refinement.
