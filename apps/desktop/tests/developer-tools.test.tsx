import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { DevSkillPackage } from "../src/renderer/api";
import App from "../src/renderer/App";
import {
  buildCreateSkillPrompt,
  buildModifySkillPrompt
} from "../src/renderer/devSkillPrompts";
import { DeveloperTools } from "../src/renderer/screens/DeveloperTools";

afterEach(() => {
  cleanup();
  window.history.replaceState(null, "", "/");
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("developer skill prompts", () => {
  it("builds a create prompt for Codex skill-creator", () => {
    const prompt = buildCreateSkillPrompt("/repo/skills");

    expect(prompt).toContain("Use the existing skill-creator skill");
    expect(prompt).toContain("/repo/skills");
    expect(prompt).toContain("SKILL.md is a development authoring asset");
    expect(prompt).toContain("skill.json is the GeneralAgent runtime contract");
  });

  it("builds a modify prompt with package diagnostics", () => {
    const skillPackage: DevSkillPackage = {
      id: "echo",
      path: "echo",
      name: "echo",
      description: "Echo a text payload.",
      hasSkillMd: false,
      hasRuntimeManifest: true,
      runtimeTools: ["echo"],
      packageKind: "runtime",
      bundleReady: true,
      validation: {
        ok: false,
        errors: ["missing SKILL.md is informational only"],
        warnings: []
      }
    };

    const prompt = buildModifySkillPrompt("/repo/skills", skillPackage);

    expect(prompt).toContain("Use the existing skill-creator skill");
    expect(prompt).toContain("/repo/skills/echo");
    expect(prompt).toContain("runtime tools: echo");
    expect(prompt).toContain("missing SKILL.md is informational only");
  });
});

describe("DeveloperTools", () => {
  it("routes #developer to the developer tools screen", async () => {
    window.history.replaceState(null, "", "/#developer");
    mockFetch([
      jsonResponse({
        root: "/repo/skills",
        packages: []
      })
    ]);

    render(<App />);

    expect(
      await screen.findByRole("main", { name: "Developer Tools" })
    ).toBeInTheDocument();
  });

  it("shows settings developer entry only when the dev API is available", async () => {
    const user = userEvent.setup();
    mockFetch([
      jsonResponse({ root: "/repo/skills", packages: [] }),
      jsonResponse({ root: "/repo/skills", packages: [] })
    ]);

    render(<App />);

    await user.click(screen.getByRole("button", { name: "Open settings" }));

    expect(
      await screen.findByRole("button", { name: "Open developer tools" })
    ).toBeInTheDocument();
  });

  it("hides settings developer entry when the dev API is unavailable", async () => {
    mockFetch([new Response(JSON.stringify({ error: "not found" }), { status: 404 })]);

    render(<App />);

    await userEvent.click(screen.getByRole("button", { name: "Open settings" }));

    await waitFor(() => {
      expect(
        screen.queryByRole("button", { name: "Open developer tools" })
      ).not.toBeInTheDocument();
    });
  });

  it("treats runtime-only missing SKILL.md diagnostics as informational", async () => {
    mockFetch([
      jsonResponse({
        root: "/repo/skills",
        packages: [
          {
            id: "echo",
            path: "echo",
            name: "echo",
            description: "Echo a text payload.",
            hasSkillMd: false,
            hasRuntimeManifest: true,
            runtimeTools: ["echo"],
            packageKind: "runtime",
            bundleReady: true,
            validation: {
              ok: false,
              errors: ["missing SKILL.md is informational only"],
              warnings: []
            }
          }
        ]
      })
    ]);

    render(<DeveloperTools onBack={() => undefined} />);

    expect(await screen.findByRole("heading", { name: "Skill packages" })).toBeInTheDocument();
    expect(screen.getAllByText("Runtime only")).toHaveLength(2);
    expect(screen.getByText("SKILL.md missing")).toBeInTheDocument();
    expect(screen.queryByText("Validation issues")).not.toBeInTheDocument();
    expect(screen.queryByText("Needs attention")).not.toBeInTheDocument();
  });

  it("renders package inventory and selected runtime-only details", async () => {
    mockFetch([
      jsonResponse({
        root: "/repo/skills",
        packages: [
          {
            id: "echo",
            path: "echo",
            name: "echo",
            description: "Echo a text payload.",
            hasSkillMd: false,
            hasRuntimeManifest: true,
            runtimeTools: ["echo"],
            packageKind: "runtime",
            bundleReady: true,
            validation: { ok: true, errors: [], warnings: [] }
          }
        ]
      })
    ]);

    render(<DeveloperTools onBack={() => undefined} />);

    expect(await screen.findByRole("heading", { name: "Skill packages" })).toBeInTheDocument();
    const list = screen.getByRole("list", { name: "Skill packages" });
    expect(within(list).getByRole("button", { name: /echo/i })).toBeInTheDocument();
    expect(screen.getByText("skills/echo")).toBeInTheDocument();
    expect(screen.getByText("SKILL.md missing")).toBeInTheDocument();
    expect(screen.queryByText("Broken")).not.toBeInTheDocument();
  });

  it("shows a disabled state when the development API is unavailable", async () => {
    mockFetch([new Response(JSON.stringify({ error: "not found" }), { status: 404 })]);

    render(<DeveloperTools onBack={() => undefined} />);

    expect(
      await screen.findByText("Development API is not available")
    ).toBeInTheDocument();
  });

  it("opens a skill-creator prompt dialog for a selected package", async () => {
    const user = userEvent.setup();
    mockFetch([
      jsonResponse({
        root: "/repo/skills",
        packages: [
          {
            id: "echo",
            path: "echo",
            name: "echo",
            description: "Echo a text payload.",
            hasSkillMd: false,
            hasRuntimeManifest: true,
            runtimeTools: ["echo"],
            packageKind: "runtime",
            bundleReady: true,
            validation: { ok: true, errors: [], warnings: [] }
          }
        ]
      })
    ]);

    render(<DeveloperTools onBack={() => undefined} />);

    await user.click(
      await screen.findByRole("button", { name: "Modify with skill-creator" })
    );

    const dialog = screen.getByRole("dialog", { name: "skill-creator prompt" });
    expect(dialog).toBeInTheDocument();
    expect(
      within(dialog).getByText(/Use the existing skill-creator skill/)
    ).toBeInTheDocument();
  });

  it("deletes a package after confirmation and refreshes inventory", async () => {
    const user = userEvent.setup();
    const fetchMock = mockFetch([
      jsonResponse({
        root: "/repo/skills",
        packages: [
          {
            id: "echo",
            path: "echo",
            name: "echo",
            description: "Echo a text payload.",
            hasSkillMd: false,
            hasRuntimeManifest: true,
            runtimeTools: ["echo"],
            packageKind: "runtime",
            bundleReady: true,
            validation: { ok: true, errors: [], warnings: [] }
          }
        ]
      }),
      jsonResponse({ root: "/repo/skills", packages: [] })
    ]);

    render(<DeveloperTools onBack={() => undefined} />);

    await user.click(await screen.findByRole("button", { name: "Delete package" }));
    await user.click(screen.getByRole("button", { name: "Delete echo" }));

    await waitFor(() => {
      expect(screen.getByText("No skill packages found")).toBeInTheDocument();
    });
    expect(fetchMock).toHaveBeenLastCalledWith(
      "http://127.0.0.1:49321/dev/skills/echo",
      expect.objectContaining({ method: "DELETE" })
    );
  });

  it("keeps the current inventory visible when reloading diagnostics fails", async () => {
    const user = userEvent.setup();
    mockFetch([
      jsonResponse({
        root: "/repo/skills",
        packages: [
          {
            id: "echo",
            path: "echo",
            name: "echo",
            description: "Echo a text payload.",
            hasSkillMd: false,
            hasRuntimeManifest: true,
            runtimeTools: ["echo"],
            packageKind: "runtime",
            bundleReady: true,
            validation: { ok: true, errors: [], warnings: [] }
          }
        ]
      }),
      new Response(JSON.stringify({ error: "reload failed" }), { status: 500 })
    ]);

    render(<DeveloperTools onBack={() => undefined} />);

    await screen.findByRole("button", { name: "Modify with skill-creator" });
    await user.click(screen.getByRole("button", { name: "Reload diagnostics" }));

    expect(await screen.findByText("Action failed. Keep the current inventory and try again.")).toBeInTheDocument();
    expect(screen.getAllByText("echo").length).toBeGreaterThan(0);
    expect(screen.queryByText("Development API is not available")).not.toBeInTheDocument();
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
