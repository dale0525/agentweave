import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { Chat } from "../src/renderer/screens/Chat";

describe("Chat", () => {
  it("adds a typed user message after send", async () => {
    const user = userEvent.setup();

    render(<Chat />);

    await user.type(screen.getByLabelText("Message agent"), "Run the renderer smoke test");
    await user.click(screen.getByRole("button", { name: "Send message" }));

    expect(screen.getByText("Run the renderer smoke test")).toBeInTheDocument();
  });
});
