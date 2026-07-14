// @vitest-environment node

import { describe, expect, it, vi } from "vitest";

import { configureRequesterWindowSecurity } from "../src/main/requesterWindowSecurity";

describe("requester window security", () => {
  it("locks production file navigation to the exact trusted document", () => {
    const webContents = fakeWebContents();
    configureRequesterWindowSecurity({
      openExternal: vi.fn(),
      trustedUrl: "file:///Applications/AgentWeave/dist/index.html",
      webContents
    });

    expect(navigate(webContents, "file:///Applications/AgentWeave/dist/index.html")).toBe(false);
    expect(navigate(webContents, "file:///Applications/AgentWeave/dist/approval.html")).toBe(true);
    expect(navigate(webContents, "https://example.com/")).toBe(true);
  });

  it("allows only the configured development origin and denies renderer windows", async () => {
    const webContents = fakeWebContents();
    const openExternal = vi.fn(async () => undefined);
    configureRequesterWindowSecurity({
      openExternal,
      trustedUrl: "http://127.0.0.1:4173/",
      webContents
    });

    expect(navigate(webContents, "http://127.0.0.1:4173/owner.html")).toBe(false);
    expect(navigate(webContents, "http://localhost:4173/")).toBe(true);
    expect(webContents.openHandler?.({ url: "https://docs.example.com/guide" })).toEqual({
      action: "deny"
    });
    await Promise.resolve();
    expect(openExternal).toHaveBeenCalledWith("https://docs.example.com/guide");

    expect(webContents.openHandler?.({ url: "javascript:alert(1)" })).toEqual({ action: "deny" });
    expect(webContents.openHandler?.({ url: "file:///tmp/untrusted.html" })).toEqual({ action: "deny" });
    expect(openExternal).toHaveBeenCalledTimes(1);
  });
});

type NavigationEvent = { preventDefault(): void };
type FakeWebContents = {
  navigationHandler?: (event: NavigationEvent, url: string) => void;
  openHandler?: (details: { url: string }) => { action: "deny" };
  on(event: "will-navigate", handler: (event: NavigationEvent, url: string) => void): void;
  setWindowOpenHandler(handler: (details: { url: string }) => { action: "deny" }): void;
};

function fakeWebContents(): FakeWebContents {
  return {
    on(_event, handler) {
      this.navigationHandler = handler;
    },
    setWindowOpenHandler(handler) {
      this.openHandler = handler;
    }
  };
}

function navigate(webContents: FakeWebContents, url: string): boolean {
  let prevented = false;
  webContents.navigationHandler?.({ preventDefault: () => { prevented = true; } }, url);
  return prevented;
}
