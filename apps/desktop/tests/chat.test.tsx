import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";

import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import App from "../src/renderer/App";
import { Chat } from "../src/renderer/screens/Chat";

describe("Chat", () => {
  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    window.history.replaceState(null, "", "/");
  });

  it("creates a session, posts a user message, and displays the assistant response", async () => {
    const user = userEvent.setup();
    const fetchMock = mockFetch([
      jsonResponse({ id: "session-1", title: "Provider adapter MVP" }),
      jsonResponse({
        accepted: true,
        assistant_message: {
          id: "assistant-1",
          role: "assistant",
          content: "MVP agent received: Run the renderer smoke test"
        },
        events: [
          { type: "turn_started", turn_id: "turn-1" },
          {
            type: "assistant_message_finished",
            text: "MVP agent received: Run the renderer smoke test"
          },
          { type: "turn_finished", turn_id: "turn-1" }
        ]
      })
    ]);

    render(<Chat />);

    await user.type(
      screen.getByLabelText("Message GeneralAgent"),
      "Run the renderer smoke test"
    );
    await user.click(screen.getByRole("button", { name: "Send message" }));

    expect(screen.getByText("Run the renderer smoke test")).toBeInTheDocument();
    expect(screen.getByLabelText("Message GeneralAgent")).toHaveValue("");
    expect(
      await screen.findByText("MVP agent received: Run the renderer smoke test")
    ).toBeInTheDocument();
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(fetchMock).toHaveBeenNthCalledWith(
      1,
      "http://127.0.0.1:49321/sessions",
      expect.objectContaining({
        body: JSON.stringify({ title: "Provider adapter MVP" }),
        method: "POST"
      })
    );
    expect(fetchMock).toHaveBeenNthCalledWith(
      2,
      "http://127.0.0.1:49321/sessions/session-1/messages",
      expect.objectContaining({
        body: JSON.stringify({ content: "Run the renderer smoke test" }),
        method: "POST"
      })
    );
  });

  it("ignores blank messages", async () => {
    const user = userEvent.setup();
    const fetchMock = mockFetch([]);

    render(<Chat />);

    await user.type(screen.getByLabelText("Message GeneralAgent"), "   ");
    await user.click(screen.getByRole("button", { name: "Send message" }));

    expect(screen.queryByText("you")).not.toBeInTheDocument();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("shows an inline server error when posting fails", async () => {
    const user = userEvent.setup();
    mockFetch([
      jsonResponse({ id: "session-1", title: "Provider adapter MVP" }),
      new Response(JSON.stringify({ error: "boom" }), {
        headers: { "Content-Type": "application/json" },
        status: 500,
        statusText: "Internal Server Error"
      })
    ]);

    render(<Chat />);

    await user.type(
      screen.getByLabelText("Message GeneralAgent"),
      "Trigger an API failure"
    );
    await user.click(screen.getByRole("button", { name: "Send message" }));

    expect(screen.getByText("Trigger an API failure")).toBeInTheDocument();
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Could not send message. Check your model or service connection."
    );
  });

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

  it("opens and closes the conversation drawer", async () => {
    const user = userEvent.setup();

    render(<Chat />);

    await user.click(screen.getByRole("button", { name: "Open conversations" }));

    expect(
      screen.getByRole("dialog", { name: "Conversations" })
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "New chat" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Close conversations" }));

    expect(
      screen.queryByRole("dialog", { name: "Conversations" })
    ).not.toBeInTheDocument();
  });

  it("starts a new local conversation from the drawer", async () => {
    const user = userEvent.setup();

    render(<Chat />);

    await user.type(screen.getByLabelText("Message GeneralAgent"), "Clear this");
    await user.click(screen.getByRole("button", { name: "Send message" }));
    expect(screen.getByText("Clear this")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Open conversations" }));
    await user.click(screen.getByRole("button", { name: "New chat" }));

    expect(screen.getByText("Hello! How can I help you today?")).toBeInTheDocument();
    expect(screen.queryByText("Clear this")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("dialog", { name: "Conversations" })
    ).not.toBeInTheDocument();
  });

  it("ignores an in-flight assistant response after starting a new chat", async () => {
    const user = userEvent.setup();
    const pendingMessage = createDeferred<Response>();
    mockFetch([
      jsonResponse({ id: "session-1", title: "Provider adapter MVP" }),
      pendingMessage.promise
    ]);

    render(<Chat />);

    await user.type(screen.getByLabelText("Message GeneralAgent"), "Old request");
    await user.click(screen.getByRole("button", { name: "Send message" }));
    await screen.findByText("Old request");

    await user.click(screen.getByRole("button", { name: "Open conversations" }));
    await user.click(screen.getByRole("button", { name: "New chat" }));

    pendingMessage.resolve(
      jsonResponse({
        accepted: true,
        assistant_message: {
          content: "Stale assistant response",
          created_at: "2026-06-25T00:00:00.000Z",
          id: "assistant-1",
          role: "assistant",
          session_id: "session-1"
        }
      })
    );

    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Send message" })).toBeEnabled();
    });
    expect(screen.getByText("Hello! How can I help you today?")).toBeInTheDocument();
    expect(screen.queryByText("Old request")).not.toBeInTheDocument();
    expect(screen.queryByText("Stale assistant response")).not.toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("keeps the consumer chat layout classes styled", () => {
    const css = readCssBundle("src/renderer/styles/index.css");
    const entryCss = readCssBundle("src/renderer/styles/index.css", {
      inlineImports: false
    });

    expect(entryCss).toContain('@import "./tokens.css";');
    expect(entryCss).toContain('@import "./base.css";');
    expect(entryCss).toContain('@import "./chat.css";');
    expect(entryCss).toContain('@import "./drawer.css";');
    expect(entryCss).toContain('@import "./settings.css";');
    expect(css).toMatch(/--color-primary:\s*#0d9488/);
    expect(css).toMatch(/\.chat-shell[\s\S]*?\{/);
    expect(css).toMatch(/\.top-bar[\s\S]*?\{/);
    expect(css).toMatch(/\.top-bar-title[\s\S]*?\{/);
    expect(css).toMatch(/\.message-list[\s\S]*?\{/);
    expect(css).toMatch(/\.message-bubble[\s\S]*?\{/);
    expect(css).toMatch(/\.message-bubble-assistant[\s\S]*?\{/);
    expect(css).toMatch(/\.message-bubble-user[\s\S]*?\{/);
    expect(css).toMatch(
      /@media \(max-width: 640px\)[\s\S]*\.composer[\s\S]*position: fixed/
    );
  });
});

describe("App navigation", () => {
  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    window.history.replaceState(null, "", "/");
  });

  it("updates the active view when the location hash changes", async () => {
    window.history.replaceState(null, "", "/#settings");

    render(<App />);

    expect(screen.getByRole("heading", { name: "Settings" })).toBeInTheDocument();

    window.history.replaceState(null, "", "/");
    window.dispatchEvent(new HashChangeEvent("hashchange"));

    await waitFor(() => {
      expect(screen.getByLabelText("Message GeneralAgent")).toBeInTheDocument();
    });
  });

  it("opens sessions from the location hash", async () => {
    window.history.replaceState(null, "", "/#sessions");

    render(<App />);

    expect(screen.getByRole("heading", { name: "Sessions" })).toBeInTheDocument();
  });

  it("opens settings from chat and returns to chat", async () => {
    const user = userEvent.setup();

    render(<App />);

    await user.click(screen.getByRole("button", { name: "Open settings" }));

    expect(screen.getByRole("heading", { name: "Settings" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Back to chat" }));

    expect(screen.getByLabelText("Message GeneralAgent")).toBeInTheDocument();
  });

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

  it("keeps model connection testing static in the renderer", async () => {
    const user = userEvent.setup();
    const fetchMock = mockFetch([]);
    window.history.replaceState(null, "", "/#settings");

    render(<App />);

    expect(screen.getByText("Connection: Not tested")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Test connection" }));

    expect(screen.getByText("Connection: Not tested")).toBeInTheDocument();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("keeps settings styles free of user-facing skill selectors", () => {
    const css = readCssBundle("src/renderer/styles/index.css");

    expect(css).toMatch(/\.conversation-drawer-content[\s\S]*?\{/);
    expect(css).toMatch(/\.settings-shell[\s\S]*?\{/);
    expect(css).toMatch(/\.settings-panel[\s\S]*?\{/);
    expect(css).not.toMatch(/\.settings-skill-row/);
    expect(css).not.toMatch(/\.skill-switch/);
    expect(css).not.toMatch(/(^|[,{}]\s*)\.skill-row(?:\s|,|\{)/m);
  });

  it("keeps sessions available only through the location hash", () => {
    render(<App />);

    expect(
      screen.queryByRole("button", { name: "Open sessions" })
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Open conversations" })
    ).toBeInTheDocument();
    expect(screen.getByLabelText("Message GeneralAgent")).toBeInTheDocument();
  });
});

function jsonResponse(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    headers: { "Content-Type": "application/json" },
    status: 200
  });
}

function mockFetch(responses: Array<Response | Promise<Response>>) {
  const fetchMock = vi.fn();
  for (const response of responses) {
    fetchMock.mockResolvedValueOnce(response);
  }
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

function readCssBundle(
  entryPath: string,
  options: { inlineImports?: boolean } = {}
): string {
  const { inlineImports = true } = options;
  const css = readFileSync(entryPath, "utf8");
  const imports = [...css.matchAll(/@import\s+"([^"]+)";/g)];
  if (!inlineImports || imports.length === 0) {
    return css;
  }

  return imports
    .map((match) => readCssBundle(join(dirname(entryPath), match[1])))
    .join("\n");
}

function createDeferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((promiseResolve, promiseReject) => {
    resolve = promiseResolve;
    reject = promiseReject;
  });

  return { promise, reject, resolve };
}
