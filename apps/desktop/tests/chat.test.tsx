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

    await user.type(screen.getByLabelText("Message agent"), "Run the renderer smoke test");
    await user.click(screen.getByRole("button", { name: "Send message" }));

    expect(screen.getByText("Run the renderer smoke test")).toBeInTheDocument();
    expect(screen.getByLabelText("Message agent")).toHaveValue("");
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

    await user.type(screen.getByLabelText("Message agent"), "   ");
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

    await user.type(screen.getByLabelText("Message agent"), "Trigger an API failure");
    await user.click(screen.getByRole("button", { name: "Send message" }));

    expect(screen.getByText("Trigger an API failure")).toBeInTheDocument();
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Could not send message: boom"
    );
  });
});

describe("App navigation", () => {
  afterEach(() => {
    cleanup();
    window.history.replaceState(null, "", "/");
  });

  it("updates the active view when the location hash changes", async () => {
    window.history.replaceState(null, "", "/#sessions");

    render(<App />);

    expect(screen.getByRole("heading", { name: "Sessions" })).toBeInTheDocument();

    window.history.replaceState(null, "", "/");
    window.dispatchEvent(new HashChangeEvent("hashchange"));

    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "Provider adapter MVP" })).toBeInTheDocument();
    });
  });

  it("provides a mobile sessions control that returns to chat", async () => {
    const user = userEvent.setup();
    window.history.replaceState(null, "", "/#sessions");

    render(<App />);

    await user.click(screen.getByRole("button", { name: "Back to chat" }));

    expect(screen.getByRole("heading", { name: "Provider adapter MVP" })).toBeInTheDocument();
  });
});

function jsonResponse(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    headers: { "Content-Type": "application/json" },
    status: 200
  });
}

function mockFetch(responses: Response[]) {
  const fetchMock = vi.fn();
  for (const response of responses) {
    fetchMock.mockResolvedValueOnce(response);
  }
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}
