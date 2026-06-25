# Consumer Chat UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> Superseded note: The Skills settings tab described in this plan is no longer a valid product target. Use `docs/superpowers/specs/2026-06-25-developer-agent-framework-repositioning-design.md` and `docs/superpowers/plans/2026-06-25-developer-agent-framework-repositioning.md` for the current implementation direction.

**Goal:** Replace the current developer workbench renderer with a consumer-friendly chat-first UI that has a hidden conversation drawer and a settings view for model connection and skill management.

**Architecture:** Keep the existing React/Vite/Electron renderer and server chat API. Use Radix UI primitives for drawer, tabs, and switches; split chat, drawer, settings, fixtures, and styles into focused files so no source file approaches 1000 physical lines.

**Tech Stack:** React 18, TypeScript, Vite, Vitest, Testing Library, lucide-react, Radix UI Dialog/Tabs/Switch, CSS variables with `prefers-color-scheme`, pixi-managed local Node.

---

## Stitch Source Of Truth

Use Stitch project `projects/8616130577965446903` (`GeneralAgent Task 10 MVP Chat Verification`) and design system asset `assets/e4d441befa1d42e4af22f64b6d8e5d3c`.

Codex reviewed the generated screen metadata and screenshot references. Use these screens as the implementation target:

| View | Device | Stitch screen ID | Route or state |
| --- | --- | --- | --- |
| Chat | Desktop | `461eff16a7494012ad9524538fbb0a51` | `#/` |
| Chat | Mobile | `167503d23b82470c8d94be18327febb8` | `#/` at 390px width |
| Settings Model | Desktop | `649a9bb196474ef59732a3c3f0d1f9d7` | `#settings`, Model tab |
| Settings Model | Mobile | `a57913d00fce464a8c06befeef388c08` | `#settings`, Model tab at 390px width |
| Settings Skills | Desktop | `7a290ad8d9d5450cafae27b8f5c436fd` | `#settings`, Skills tab |
| Settings Skills | Mobile | `86ab488e0e8f47c9bc4ae489798efe96` | `#settings`, Skills tab at 390px width |
| Conversation Drawer | Desktop | `acfff82e35404878ba881ddddeb6788e` | Chat with history drawer open |
| Conversation Drawer | Mobile | `e5b849ed880e4170b77d8f6d5ad54284` | Chat with history sheet open at 390px width |

Implementation notes from Stitch review:

- Preserve the chat-first shape: compact top bar, centered message column, bottom composer.
- Remove permanent session rail, inspector, runtime logs, token counters, raw tool-call output, model chips, and developer status labels from the primary UI.
- Use teal `#0d9488` as the only strong action accent, neutral light and dark surfaces, 4px radius for controls, and 8px maximum radius for message bubbles, settings groups, drawer rows, and skill rows.
- Stitch generated light-mode screens. Implement both light and dark tokens with `prefers-color-scheme`; visual comparison should use light mode first, then verify dark mode for contrast and layout.

## File Structure

Create or modify these files:

```text
apps/desktop/
├── package.json
├── package-lock.json
├── src/renderer/
│   ├── App.tsx
│   ├── main.tsx
│   ├── api.ts
│   ├── data/
│   │   └── fixtures.ts
│   ├── components/
│   │   ├── AppIconButton.tsx
│   │   ├── Composer.tsx
│   │   ├── ConversationDrawer.tsx
│   │   ├── MessageList.tsx
│   │   ├── SettingsModel.tsx
│   │   └── SettingsSkills.tsx
│   ├── screens/
│   │   ├── Chat.tsx
│   │   └── Settings.tsx
│   ├── styles/
│   │   ├── index.css
│   │   ├── tokens.css
│   │   ├── base.css
│   │   ├── chat.css
│   │   ├── drawer.css
│   │   └── settings.css
│   └── types.ts
└── tests/
    └── chat.test.tsx
```

Responsibilities:

- `App.tsx`: hash-based `chat` and `settings` routing.
- `types.ts`: renderer-only UI types for messages, conversations, settings form, and skills.
- `data/fixtures.ts`: starter messages, conversation rows, and skill rows.
- `Chat.tsx`: server-backed send flow, drawer state, and consumer chat screen composition.
- `ConversationDrawer.tsx`: Radix Dialog wrapper for desktop drawer and mobile sheet.
- `Settings.tsx`: settings route and Radix Tabs state.
- `SettingsModel.tsx`: endpoint type, base URL, API key, model name, and test status UI.
- `SettingsSkills.tsx`: skill rows with Radix Switch state.
- `styles/*.css`: design tokens, layout, chat, drawer, and settings styling. Keep each file small and scoped.

## Task 1: Add UI Primitives And Route Contract

**Files:**

- Modify: `apps/desktop/package.json`
- Modify: `apps/desktop/package-lock.json`
- Modify: `apps/desktop/src/renderer/App.tsx`
- Create: `apps/desktop/src/renderer/screens/Settings.tsx`
- Create: `apps/desktop/src/renderer/components/AppIconButton.tsx`
- Modify: `apps/desktop/tests/chat.test.tsx`

- [ ] **Step 1: Add Radix dependencies**

Run:

```bash
pixi run npm --prefix apps/desktop install @radix-ui/react-dialog @radix-ui/react-switch @radix-ui/react-tabs
```

Expected: `apps/desktop/package.json` and `apps/desktop/package-lock.json` include the three Radix packages.

- [ ] **Step 2: Write failing navigation test**

Add this test to `describe("App navigation", ...)` in `apps/desktop/tests/chat.test.tsx`:

```tsx
it("opens settings from chat and returns to chat", async () => {
  const user = userEvent.setup();

  render(<App />);

  await user.click(screen.getByRole("button", { name: "Open settings" }));

  expect(screen.getByRole("heading", { name: "Settings" })).toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Back to chat" }));

  expect(screen.getByLabelText("Message GeneralAgent")).toBeInTheDocument();
});
```

- [ ] **Step 3: Run test to verify it fails**

Run:

```bash
pixi run npm --prefix apps/desktop test
```

Expected: FAIL because the chat screen has no `Open settings` control and `Settings` route does not exist.

- [ ] **Step 4: Create shared icon button**

Create `apps/desktop/src/renderer/components/AppIconButton.tsx`:

```tsx
import { ReactNode } from "react";

type AppIconButtonProps = {
  children: ReactNode;
  disabled?: boolean;
  label: string;
  onClick?: () => void;
  type?: "button" | "submit";
};

export function AppIconButton({
  children,
  disabled = false,
  label,
  onClick,
  type = "button"
}: AppIconButtonProps): JSX.Element {
  return (
    <button
      aria-label={label}
      className="icon-button"
      disabled={disabled}
      onClick={onClick}
      title={label}
      type={type}
    >
      {children}
    </button>
  );
}
```

- [ ] **Step 5: Create settings route shell**

Create `apps/desktop/src/renderer/screens/Settings.tsx`:

```tsx
import { ArrowLeft } from "lucide-react";

import { AppIconButton } from "../components/AppIconButton";

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
      <section className="settings-shell">
        <h2>Model</h2>
      </section>
    </main>
  );
}
```

- [ ] **Step 6: Update App routing**

Replace `apps/desktop/src/renderer/App.tsx` with:

```tsx
import { useEffect, useState } from "react";

import { Chat } from "./screens/Chat";
import { Settings } from "./screens/Settings";

type AppView = "chat" | "settings";

function getViewFromHash(): AppView {
  if (typeof window !== "undefined" && window.location.hash === "#settings") {
    return "settings";
  }

  return "chat";
}

export default function App(): JSX.Element {
  const [view, setView] = useState<AppView>(getViewFromHash);

  useEffect(() => {
    const syncViewFromHash = () => setView(getViewFromHash());

    window.addEventListener("hashchange", syncViewFromHash);

    return () => window.removeEventListener("hashchange", syncViewFromHash);
  }, []);

  const navigate = (nextView: AppView) => {
    setView(nextView);
    const nextHash = nextView === "settings" ? "#settings" : "";
    if (typeof window !== "undefined" && window.location.hash !== nextHash) {
      window.location.hash = nextHash;
    }
  };

  return (
    <div className="app-root">
      {view === "settings" ? (
        <Settings onBack={() => navigate("chat")} />
      ) : (
        <Chat onOpenSettings={() => navigate("settings")} />
      )}
    </div>
  );
}
```

- [ ] **Step 7: Run tests**

Run:

```bash
pixi run npm --prefix apps/desktop test
```

Expected: Navigation test still fails until `Chat` exposes `Open settings`; existing chat tests may also fail because labels change in later tasks.

- [ ] **Step 8: Commit route contract**

Run:

```bash
git add apps/desktop/package.json apps/desktop/package-lock.json apps/desktop/src/renderer/App.tsx apps/desktop/src/renderer/screens/Settings.tsx apps/desktop/src/renderer/components/AppIconButton.tsx apps/desktop/tests/chat.test.tsx
git commit -m "feat: add consumer settings route shell"
```

## Task 2: Refactor Chat Into Consumer Components

**Files:**

- Create: `apps/desktop/src/renderer/types.ts`
- Create: `apps/desktop/src/renderer/data/fixtures.ts`
- Create: `apps/desktop/src/renderer/components/MessageList.tsx`
- Create: `apps/desktop/src/renderer/components/Composer.tsx`
- Modify: `apps/desktop/src/renderer/screens/Chat.tsx`
- Modify: `apps/desktop/tests/chat.test.tsx`

- [ ] **Step 1: Update chat tests for consumer labels and plain error copy**

In `apps/desktop/tests/chat.test.tsx`, change chat input label assertions from `Message agent` to `Message GeneralAgent`, and change the error assertion to:

```tsx
expect(await screen.findByRole("alert")).toHaveTextContent(
  "Could not send message. Check your model or service connection."
);
```

- [ ] **Step 2: Add renderer types**

Create `apps/desktop/src/renderer/types.ts`:

```tsx
export type ChatMessage = {
  body: string;
  id: string;
  role: "assistant" | "user";
};

export type ConversationSummary = {
  id: string;
  title: string;
  updatedAt: string;
};

export type EndpointType = "responses" | "chat_completions" | "completion";

export type ModelSettings = {
  apiKey: string;
  baseUrl: string;
  endpointType: EndpointType;
  modelName: string;
};

export type SkillStatus = "active" | "inactive" | "unavailable";

export type SkillSummary = {
  description: string;
  enabled: boolean;
  id: string;
  name: string;
  status: SkillStatus;
};
```

- [ ] **Step 3: Add static fixtures**

Create `apps/desktop/src/renderer/data/fixtures.ts`:

```tsx
import { ChatMessage, ConversationSummary, SkillSummary } from "../types";

export const starterMessages: ChatMessage[] = [
  {
    body: "Hello! How can I help you today?",
    id: "starter-assistant",
    role: "assistant"
  }
];

export const conversations: ConversationSummary[] = [
  { id: "new", title: "New conversation", updatedAt: "Just now" },
  { id: "trip", title: "Trip planning", updatedAt: "2 hours ago" },
  { id: "draft", title: "Draft reply", updatedAt: "Yesterday" },
  { id: "research", title: "Research notes", updatedAt: "Oct 24" }
];

export const skills: SkillSummary[] = [
  {
    description: "Quickly read and organize your uploaded files.",
    enabled: true,
    id: "file-helper",
    name: "File Helper",
    status: "active"
  },
  {
    description: "Browse the web to find the latest information and answers.",
    enabled: true,
    id: "web-research",
    name: "Web Research",
    status: "active"
  },
  {
    description: "Schedule events and check your availability.",
    enabled: false,
    id: "calendar",
    name: "Calendar",
    status: "inactive"
  },
  {
    description: "Run local actions when the desktop bridge supports them.",
    enabled: false,
    id: "local-command",
    name: "Local Command",
    status: "unavailable"
  }
];
```

- [ ] **Step 4: Create MessageList**

Create `apps/desktop/src/renderer/components/MessageList.tsx`:

```tsx
import { ChatMessage } from "../types";

type MessageListProps = {
  messages: ChatMessage[];
};

export function MessageList({ messages }: MessageListProps): JSX.Element {
  return (
    <section className="message-list" aria-live="polite" aria-label="Conversation">
      {messages.map((message) => (
        <article
          className={`message-bubble message-bubble-${message.role}`}
          key={message.id}
        >
          <p>{message.body}</p>
        </article>
      ))}
    </section>
  );
}
```

- [ ] **Step 5: Create Composer**

Create `apps/desktop/src/renderer/components/Composer.tsx`:

```tsx
import { FormEvent } from "react";
import { Send } from "lucide-react";

import { AppIconButton } from "./AppIconButton";

type ComposerProps = {
  draft: string;
  error: string | null;
  isSending: boolean;
  onChange: (value: string) => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
};

export function Composer({
  draft,
  error,
  isSending,
  onChange,
  onSubmit
}: ComposerProps): JSX.Element {
  return (
    <form aria-label="Message composer" className="composer" onSubmit={onSubmit}>
      {error ? (
        <p className="composer-error" role="alert">
          {error}
        </p>
      ) : null}
      <div className="composer-input-row">
        <label className="sr-only" htmlFor="generalagent-message">
          Message GeneralAgent
        </label>
        <input
          id="generalagent-message"
          aria-label="Message GeneralAgent"
          value={draft}
          onChange={(event) => onChange(event.target.value)}
        />
        <AppIconButton disabled={isSending} label="Send message" type="submit">
          <Send size={18} aria-hidden="true" />
        </AppIconButton>
      </div>
    </form>
  );
}
```

- [ ] **Step 6: Replace Chat screen with consumer layout**

Replace `apps/desktop/src/renderer/screens/Chat.tsx` with a consumer chat screen that imports `Menu`, `Settings`, `createServerSession`, `postSessionMessage`, `extractAssistantText`, `starterMessages`, `Composer`, `MessageList`, and `AppIconButton`. Keep the existing create-session and post-message flow. Use this error string inside the catch block:

```tsx
setApiError("Could not send message. Check your model or service connection.");
```

The top bar must include:

```tsx
<AppIconButton label="Open conversations" onClick={() => setIsDrawerOpen(true)}>
  <Menu size={18} aria-hidden="true" />
</AppIconButton>
...
<AppIconButton label="Open settings" onClick={onOpenSettings}>
  <Settings size={18} aria-hidden="true" />
</AppIconButton>
```

- [ ] **Step 7: Run chat tests**

Run:

```bash
pixi run npm --prefix apps/desktop test
```

Expected: chat send, blank message, error copy, and settings navigation tests pass after `Chat` exposes the new labels and settings button.

- [ ] **Step 8: Commit chat refactor**

Run:

```bash
git add apps/desktop/src/renderer/types.ts apps/desktop/src/renderer/data/fixtures.ts apps/desktop/src/renderer/components/MessageList.tsx apps/desktop/src/renderer/components/Composer.tsx apps/desktop/src/renderer/screens/Chat.tsx apps/desktop/tests/chat.test.tsx
git commit -m "feat: simplify chat surface"
```

## Task 3: Add Conversation Drawer

**Files:**

- Create: `apps/desktop/src/renderer/components/ConversationDrawer.tsx`
- Modify: `apps/desktop/src/renderer/screens/Chat.tsx`
- Modify: `apps/desktop/tests/chat.test.tsx`

- [ ] **Step 1: Add drawer tests**

Add tests:

```tsx
it("opens and closes the conversation drawer", async () => {
  const user = userEvent.setup();

  render(<Chat />);

  await user.click(screen.getByRole("button", { name: "Open conversations" }));

  expect(screen.getByRole("dialog", { name: "Conversations" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "New chat" })).toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Close conversations" }));

  expect(screen.queryByRole("dialog", { name: "Conversations" })).not.toBeInTheDocument();
});

it("starts a new local conversation from the drawer", async () => {
  const user = userEvent.setup();

  render(<Chat />);

  await user.click(screen.getByRole("button", { name: "Open conversations" }));
  await user.click(screen.getByRole("button", { name: "New chat" }));

  expect(screen.getByText("Hello! How can I help you today?")).toBeInTheDocument();
  expect(screen.queryByRole("dialog", { name: "Conversations" })).not.toBeInTheDocument();
});
```

- [ ] **Step 2: Create Radix drawer**

Create `apps/desktop/src/renderer/components/ConversationDrawer.tsx` using `@radix-ui/react-dialog`. Render `Dialog.Content` with `aria-label="Conversations"`, a close icon button labeled `Close conversations`, a `New chat` button, search input, and `conversations` from fixtures.

- [ ] **Step 3: Wire drawer into Chat**

In `Chat.tsx`, add `isDrawerOpen` state and render:

```tsx
<ConversationDrawer
  isOpen={isDrawerOpen}
  onNewChat={handleNewChat}
  onOpenChange={setIsDrawerOpen}
/>
```

`handleNewChat` must reset messages to `starterMessages`, set `sessionId` to `null`, clear `apiError`, and close the drawer.

- [ ] **Step 4: Run drawer tests**

Run:

```bash
pixi run npm --prefix apps/desktop test
```

Expected: drawer tests pass; no existing chat tests regress.

- [ ] **Step 5: Commit drawer**

Run:

```bash
git add apps/desktop/src/renderer/components/ConversationDrawer.tsx apps/desktop/src/renderer/screens/Chat.tsx apps/desktop/tests/chat.test.tsx
git commit -m "feat: add conversation drawer"
```

## Task 4: Build Settings Model And Skills Tabs

**Files:**

- Create: `apps/desktop/src/renderer/components/SettingsModel.tsx`
- Create: `apps/desktop/src/renderer/components/SettingsSkills.tsx`
- Modify: `apps/desktop/src/renderer/screens/Settings.tsx`
- Modify: `apps/desktop/tests/chat.test.tsx`

- [ ] **Step 1: Add settings behavior tests**

Add tests:

```tsx
it("switches between model and skills settings", async () => {
  const user = userEvent.setup();
  window.history.replaceState(null, "", "/#settings");

  render(<App />);

  expect(screen.getByLabelText("Base URL")).toBeInTheDocument();

  await user.click(screen.getByRole("tab", { name: "Skills" }));

  expect(screen.getByText("File Helper")).toBeInTheDocument();
  expect(screen.getByText("Web Research")).toBeInTheDocument();
});

it("toggles an available skill", async () => {
  const user = userEvent.setup();
  window.history.replaceState(null, "", "/#settings");

  render(<App />);

  await user.click(screen.getByRole("tab", { name: "Skills" }));
  await user.click(screen.getByRole("switch", { name: "Calendar" }));

  expect(screen.getByRole("switch", { name: "Calendar" })).toHaveAttribute(
    "aria-checked",
    "true"
  );
});
```

- [ ] **Step 2: Create model settings component**

Create `apps/desktop/src/renderer/components/SettingsModel.tsx`. Use local state initialized to:

```tsx
{
  apiKey: "",
  baseUrl: "http://127.0.0.1:11434/v1",
  endpointType: "responses",
  modelName: "local-agent-model"
}
```

Render a Radix Tabs-compatible section with endpoint buttons, labeled inputs for `Base URL`, `API key`, and `Model name`, a `Test connection` button, and status text `Connection: Not tested`.

- [ ] **Step 3: Create skills settings component**

Create `apps/desktop/src/renderer/components/SettingsSkills.tsx`. Use local state from `skills` fixture. Render each skill as a row with plain-language description, status label, and Radix Switch. Disable switching when status is `unavailable`.

- [ ] **Step 4: Replace settings shell with Radix tabs**

Modify `apps/desktop/src/renderer/screens/Settings.tsx` to use `@radix-ui/react-tabs`. Default tab is `model`; tab labels are `Model` and `Skills`; tab panels render `SettingsModel` and `SettingsSkills`.

- [ ] **Step 5: Run settings tests**

Run:

```bash
pixi run npm --prefix apps/desktop test
```

Expected: model tab, skills tab, skill toggle, settings navigation, and chat tests pass.

- [ ] **Step 6: Commit settings**

Run:

```bash
git add apps/desktop/src/renderer/components/SettingsModel.tsx apps/desktop/src/renderer/components/SettingsSkills.tsx apps/desktop/src/renderer/screens/Settings.tsx apps/desktop/tests/chat.test.tsx
git commit -m "feat: add consumer settings tabs"
```

## Task 5: Replace Workbench Styling With Stitch-Aligned Styles

**Files:**

- Modify: `apps/desktop/src/renderer/main.tsx`
- Create: `apps/desktop/src/renderer/styles/index.css`
- Create: `apps/desktop/src/renderer/styles/tokens.css`
- Create: `apps/desktop/src/renderer/styles/base.css`
- Create: `apps/desktop/src/renderer/styles/chat.css`
- Create: `apps/desktop/src/renderer/styles/drawer.css`
- Create: `apps/desktop/src/renderer/styles/settings.css`
- Delete: `apps/desktop/src/renderer/styles.css`

- [ ] **Step 1: Update stylesheet import**

Change `apps/desktop/src/renderer/main.tsx`:

```tsx
import "./styles/index.css";
```

- [ ] **Step 2: Create stylesheet entry**

Create `apps/desktop/src/renderer/styles/index.css`:

```css
@import "./tokens.css";
@import "./base.css";
@import "./chat.css";
@import "./drawer.css";
@import "./settings.css";
```

- [ ] **Step 3: Create design tokens**

Create `tokens.css` with light tokens, dark tokens inside `@media (prefers-color-scheme: dark)`, teal primary `#0d9488`, neutral backgrounds, radius variables, and no gradients.

- [ ] **Step 4: Create base styles**

Create `base.css` for global sizing, body reset, `.app-root`, `.top-bar`, `.icon-button`, `.sr-only`, focus states, and shared text/input/button basics.

- [ ] **Step 5: Create chat, drawer, and settings styles**

Create `chat.css`, `drawer.css`, and `settings.css` to match the Stitch screens:

- Chat: centered column max width around `820px`, top bar, message list, left assistant bubble, right user bubble, bottom composer.
- Drawer: fixed overlay, 320px desktop width, near-full-width mobile sheet, dim backdrop, search field, conversation rows.
- Settings: centered max width around `960px`, segmented tab styling, model form grid, skill list rows, mobile single column.

- [ ] **Step 6: Confirm source file line budgets**

Run:

```bash
wc -l apps/desktop/src/renderer/**/*.tsx apps/desktop/src/renderer/styles/*.css apps/desktop/tests/chat.test.tsx
```

Expected: every edited or created source file is under 1000 physical lines.

- [ ] **Step 7: Run tests**

Run:

```bash
pixi run npm --prefix apps/desktop test
```

Expected: PASS.

- [ ] **Step 8: Commit styles**

Run:

```bash
git add apps/desktop/src/renderer/main.tsx apps/desktop/src/renderer/styles apps/desktop/src/renderer/styles.css
git commit -m "style: apply consumer chat design"
```

## Task 6: Manual Visual Verification

**Files:**

- Modify: `docs/mvp-verification.md`

- [ ] **Step 1: Run TypeScript check**

Run:

```bash
pixi run npm --prefix apps/desktop exec tsc -- --noEmit -p tsconfig.vitest.json
```

Expected: PASS with no TypeScript errors.

- [ ] **Step 2: Run renderer tests**

Run:

```bash
pixi run npm --prefix apps/desktop test
```

Expected: PASS.

- [ ] **Step 3: Start local dev server**

Run:

```bash
pixi run npm --prefix apps/desktop run dev -- --host 127.0.0.1 --port 5173
```

Expected: Vite serves the renderer at `http://127.0.0.1:5173/`.

- [ ] **Step 4: Capture implementation screenshots**

Use the browser or Playwright skill to capture and review:

- Desktop chat at `1440x900`.
- Mobile chat at `390x844`.
- Desktop conversation drawer at `1440x900`.
- Mobile conversation drawer at `390x844`.
- Desktop settings Model and Skills tabs at `1440x900`.
- Mobile settings Model and Skills tabs at `390x844`.
- Dark mode layout and contrast using `prefers-color-scheme: dark`.

- [ ] **Step 5: Compare against Stitch**

Compare implementation screenshots against the Stitch source IDs in this plan for layout, spacing, typography scale, hierarchy, color, component state, drawer behavior, and mobile fit. Acceptable deviations:

- Browser font fallback may differ from Stitch Inter rendering.
- Real input and button text may wrap differently when viewport width is narrower than the Stitch canvas.
- Dark mode has no direct Stitch screen, so it is verified against the same structure and contrast requirements.

- [ ] **Step 6: Update verification notes**

Add a section to `docs/mvp-verification.md` named `Consumer Chat UI Verification` with:

- Stitch project and screen IDs used.
- Commands run.
- Viewports checked.
- Visual review result.
- Known acceptable deviations.
- Confirmation that every edited or created source file is under 1000 physical lines.

- [ ] **Step 7: Commit verification**

Run:

```bash
git add docs/mvp-verification.md
git commit -m "docs: verify consumer chat UI"
```

## Self Review

- Spec coverage: The plan covers chat-first UI, hidden conversation drawer, settings with Model and Skills sections, system theme tokens, continued server-backed chat send flow, static conversation and skill data, no preview route, and no developer inspector.
- Type consistency: `ChatMessage`, `ConversationSummary`, `ModelSettings`, `EndpointType`, and `SkillSummary` are defined before use; component props reference those types consistently.
- Verification coverage: automated tests cover chat send, blank messages, failure copy, drawer open/close, new chat reset, settings navigation, tabs, and skill toggles. Manual checks cover Stitch desktop/mobile parity and dark mode.
- Line budget: CSS is split and renderer components are focused so each edited or created source file stays under 1000 physical lines.
