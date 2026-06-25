import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";

import App from "../src/renderer/App";
import { Chat } from "../src/renderer/screens/Chat";

describe("Chat", () => {
  afterEach(() => {
    cleanup();
    window.history.replaceState(null, "", "/");
  });

  it("adds a typed user message after send", async () => {
    const user = userEvent.setup();

    render(<Chat />);

    await user.type(screen.getByLabelText("Message agent"), "Run the renderer smoke test");
    await user.click(screen.getByRole("button", { name: "Send message" }));

    expect(screen.getByText("Run the renderer smoke test")).toBeInTheDocument();
    expect(screen.getByLabelText("Message agent")).toHaveValue("");
  });

  it("ignores blank messages", async () => {
    const user = userEvent.setup();

    render(<Chat />);

    await user.type(screen.getByLabelText("Message agent"), "   ");
    await user.click(screen.getByRole("button", { name: "Send message" }));

    expect(screen.queryByText("you")).not.toBeInTheDocument();
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
