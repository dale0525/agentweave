import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import App from "../src/renderer/App";
import { OwnerSkills } from "../src/renderer/screens/OwnerSkills";

class TestResizeObserver implements ResizeObserver {
  disconnect(): void {}
  observe(): void {}
  unobserve(): void {}
}

beforeEach(() => {
  vi.stubGlobal("ResizeObserver", TestResizeObserver);
});

type OwnerPolicyFixture = {
  mode: "disabled" | "diagnostics_only" | "owner_only" | "organization_managed";
  actorId: string;
  grants: string[];
};

afterEach(() => {
  cleanup();
  window.history.replaceState(null, "", "/");
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("owner skill route", () => {
  it("does not show owner skills when management is disabled", async () => {
    mockOwnerBridge({ mode: "disabled", actorId: "anonymous", grants: [] });

    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "Open settings" }));

    expect(
      screen.queryByRole("button", { name: "Manage skills" })
    ).not.toBeInTheDocument();
  });

  it("routes an authorized owner to the owner skill screen", async () => {
    mockOwnerBridge({
      mode: "owner_only",
      actorId: "owner-1",
      grants: ["inspect", "create_draft", "activate", "rollback"]
    });
    mockOwnerRequest("/owner/skills", ownerInventoryFixture());

    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "Open settings" }));
    await userEvent.click(
      await screen.findByRole("button", { name: "Manage skills" })
    );

    expect(
      await screen.findByRole("main", { name: "Owner Skills" })
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("main", { name: "Developer Tools" })
    ).not.toBeInTheDocument();
  });

  it("renders settings immediately for an unauthorized direct owner route", async () => {
    window.history.replaceState(null, "", "/#owner-skills");
    mockOwnerBridge({ mode: "disabled", actorId: "anonymous", grants: [] });

    render(<App />);

    expect(screen.getByRole("main", { name: "Settings" })).toBeInTheDocument();
    expect(screen.queryByText("Calendar Operations")).not.toBeInTheDocument();
    await waitFor(() => expect(window.location.hash).toBe("#settings"));
  });
});

describe("OwnerSkills workflows", () => {
  it("requires an approval dialog before activation", async () => {
    const bridge = setupAuthorizedOwnerFixtures();

    render(<OwnerSkills onBack={() => undefined} policy={authorizedPolicy()} />);
    await userEvent.click(
      await screen.findByRole("button", { name: "Activate revision" })
    );

    const dialog = await screen.findByRole("dialog", {
      name: "Approve skill activation"
    });
    expect(within(dialog).getByText("instruction_only")).toBeInTheDocument();
    expect(within(dialog).getByText("No new capabilities")).toBeInTheDocument();
    expect(within(dialog).getByText("calendar.read")).toBeInTheDocument();
    expect(within(dialog).getByText("owner-1")).toBeInTheDocument();

    await userEvent.click(
      within(dialog).getByRole("button", { name: "Approve activation" })
    );

    expect(await screen.findByText("Active snapshot 4")).toBeInTheDocument();
    expect(bridge.ownerRequest).toHaveBeenCalledWith(
      "/owner/skills/approvals/approval-1",
      expect.objectContaining({
        body: JSON.stringify({ decision: "approve" }),
        method: "POST"
      })
    );
  });

  it("keeps the active revision visible when rollback fails", async () => {
    setupRollbackFailureFixtures();

    render(<OwnerSkills onBack={() => undefined} policy={authorizedPolicy()} />);
    await userEvent.click(
      await screen.findByRole("button", { name: "Rollback to 1.0.0" })
    );

    expect(
      await screen.findByText(
        "Rollback failed. The current revision remains active."
      )
    ).toBeInTheDocument();
    expect(screen.getByText("2.0.0 Active")).toBeInTheDocument();
  });

  it("loads revision history from the owner audit route", async () => {
    const inventory = managedInventoryFixture() as {
      effective: Array<Record<string, unknown>>;
      managed: unknown[];
    };
    delete inventory.effective[0].revisions;
    mockOwnerRequests(authorizedPolicy(), [
      route("GET", "/owner/skills", inventory),
      route("GET", "/owner/skills/com.example.calendar/audit", [
        {
          id: "audit-active",
          actor_id: "owner-1",
          operation: "activate_revision",
          package_id: "com.example.calendar",
          revision_id: "draft-active",
          result: "ok",
          metadata_json: { version: "2.0.0" },
          created_at: "2026-07-12T10:00:00Z"
        },
        {
          id: "audit-old",
          actor_id: "owner-1",
          operation: "activate_revision",
          package_id: "com.example.calendar",
          revision_id: "revision-1",
          result: "ok",
          metadata_json: { version: "1.0.0" },
          created_at: "2026-07-11T10:00:00Z"
        }
      ])
    ]);

    render(<OwnerSkills onBack={() => undefined} policy={authorizedPolicy()} />);
    await userEvent.click(await screen.findByRole("tab", { name: "Revisions" }));

    expect(
      await screen.findAllByRole("button", { name: "Rollback to 1.0.0" })
    ).not.toHaveLength(0);
  });

  it("keeps draft content after validation errors and blocks activation", async () => {
    setupDraftValidationFailureFixtures();

    render(<OwnerSkills onBack={() => undefined} policy={authorizedPolicy()} />);
    await userEvent.click(await screen.findByRole("tab", { name: "Draft" }));
    const editor = screen.getByRole("textbox", { name: "Instructions" });
    await userEvent.clear(editor);
    await userEvent.type(editor, "Keep this draft content");
    await userEvent.click(screen.getByRole("button", { name: "Validate draft" }));

    expect(await screen.findByText("Validation failed")).toBeInTheDocument();
    expect(screen.getByText("Instruction heading is required")).toBeInTheDocument();
    expect(editor).toHaveValue("Keep this draft content");
    expect(
      screen.getByRole("button", { name: "Request activation" })
    ).toBeDisabled();
  });

  it("creates, saves, validates, and requests activation for a new draft", async () => {
    const bridge = setupCreateDraftFixtures();

    render(<OwnerSkills onBack={() => undefined} policy={authorizedPolicy()} />);
    await userEvent.click(await screen.findByRole("button", { name: "New draft" }));
    await userEvent.type(screen.getByLabelText("Package ID"), "com.example.notes");
    await userEvent.type(screen.getByLabelText("Display name"), "Notes");
    await userEvent.type(screen.getByLabelText("Description"), "Capture notes.");
    await userEvent.click(screen.getByRole("button", { name: "Create draft" }));

    const editor = await screen.findByRole("textbox", { name: "Instructions" });
    await userEvent.clear(editor);
    await userEvent.type(editor, "# Notes\n\nCapture a note.");
    await userEvent.click(screen.getByRole("button", { name: "Save draft" }));
    await userEvent.click(screen.getByRole("button", { name: "Validate draft" }));

    expect(await screen.findByText("Validation passed")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Request activation" })
    ).toBeEnabled();
    expect(bridge.ownerRequest).toHaveBeenCalledWith(
      "/owner/skills/drafts/draft-new",
      expect.objectContaining({ method: "PUT" })
    );
  });

  it("disables and requests removal only for a validated managed package", async () => {
    const bridge = setupAuthorizedOwnerFixtures();

    render(<OwnerSkills onBack={() => undefined} policy={authorizedPolicy()} />);
    await userEvent.click(await screen.findByRole("button", { name: "Disable skill" }));
    expect(await screen.findByText("Skill disabled")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Remove skill" }));
    const dialog = await screen.findByRole("dialog", {
      name: "Approve skill removal"
    });
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Approve removal" })
    );

    await waitFor(() => {
      expect(bridge.ownerRequest).toHaveBeenCalledWith(
        "/owner/skills/com.example.calendar",
        expect.objectContaining({ method: "DELETE" })
      );
    });
  });

  it("hides mutation actions that are not granted", async () => {
    setupAuthorizedOwnerFixtures();

    render(
      <OwnerSkills
        onBack={() => undefined}
        policy={{
          mode: "organization_managed",
          actorId: "org-reader",
          grants: [
            "inspect",
            "create_draft",
            "edit_draft",
            "validate",
            "activate",
            "rollback",
            "disable",
            "delete_managed"
          ]
        }}
      />
    );

    expect(await screen.findAllByText("Calendar Operations")).not.toHaveLength(0);
    expect(screen.queryByRole("button", { name: "New draft" })).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Activate revision" })
    ).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Disable skill" })).not.toBeInTheDocument();
  });
});

function mockOwnerBridge(policy: OwnerPolicyFixture): void {
  Object.defineProperty(window, "generalAgent", {
    configurable: true,
    value: {
      ownerPolicy: vi.fn().mockResolvedValue(policy),
      ownerRequest: vi.fn().mockRejectedValue(new Error("Unexpected owner request"))
    }
  });
}

function authorizedPolicy(): OwnerPolicyFixture {
  return {
    mode: "owner_only",
    actorId: "owner-1",
    grants: [
      "inspect",
      "create_draft",
      "edit_draft",
      "validate",
      "activate",
      "rollback",
      "disable",
      "delete_managed"
    ]
  };
}

function setupAuthorizedOwnerFixtures(): {
  ownerPolicy: ReturnType<typeof vi.fn>;
  ownerRequest: ReturnType<typeof vi.fn>;
} {
  return mockOwnerRequests(authorizedPolicy(), [
    route("GET", "/owner/skills", managedInventoryFixture()),
    route("POST", "/owner/skills/drafts/draft-active/activation", {
      approval_id: "approval-1",
      package_id: "com.example.calendar",
      permission_diff: { capabilities: { added: [] } },
      requested_by: "owner-1",
      revision_id: "draft-active",
      status: "pending"
    }),
    route("POST", "/owner/skills/approvals/approval-1", {
      status: "approved",
      active_generation: 4
    }),
    route("POST", "/owner/skills/com.example.calendar/disable", {
      active_generation: 5
    }),
    route("DELETE", "/owner/skills/com.example.calendar", {
      approval_id: "removal-1",
      package_id: "com.example.calendar",
      permission_diff: {},
      requested_by: "owner-1",
      revision_id: "draft-active",
      status: "pending"
    }),
    route("POST", "/owner/skills/approvals/removal-1", {
      status: "approved",
      active_generation: 6
    })
  ]);
}

function setupRollbackFailureFixtures(): void {
  mockOwnerRequests(authorizedPolicy(), [
    route("GET", "/owner/skills", managedInventoryFixture()),
    route(
      "POST",
      "/owner/skills/com.example.calendar/rollback",
      new Error("conflict")
    )
  ]);
}

function setupDraftValidationFailureFixtures(): void {
  mockOwnerRequests(authorizedPolicy(), [
    route("GET", "/owner/skills", managedInventoryFixture({ draft: true })),
    route("PUT", "/owner/skills/drafts/draft-active", {
      package_id: "com.example.calendar",
      revision_id: "draft-active",
      version: "2.0.0",
      kind: "instruction_only",
      validation: { status: "pending" },
      status: "draft"
    }),
    route("POST", "/owner/skills/drafts/draft-active/validate", {
      ok: false,
      errors: ["Instruction heading is required"],
      warnings: [],
      requiredTools: [],
      requiredConnectors: [],
      dependencies: [],
      requiredCapabilities: [],
      resolverStatus: "invalid",
      resolverErrors: [],
      permissionDiff: {},
      revisionId: "draft-active",
      contentHash: "hash",
      snapshotGeneration: 3
    })
  ]);
}

function setupCreateDraftFixtures(): {
  ownerPolicy: ReturnType<typeof vi.fn>;
  ownerRequest: ReturnType<typeof vi.fn>;
} {
  return mockOwnerRequests(authorizedPolicy(), [
    route("GET", "/owner/skills", { effective: [], managed: [] }),
    route("POST", "/owner/skills/drafts", {
      package_id: "com.example.notes",
      revision_id: "draft-new",
      version: "0.1.0",
      kind: "instruction_only",
      validation: { status: "pending" },
      status: "draft"
    }),
    route("PUT", "/owner/skills/drafts/draft-new", {
      package_id: "com.example.notes",
      revision_id: "draft-new",
      version: "0.1.0",
      kind: "instruction_only",
      validation: { status: "pending" },
      status: "draft"
    }),
    route("POST", "/owner/skills/drafts/draft-new/validate", {
      ok: true,
      errors: [],
      warnings: [],
      requiredTools: [],
      requiredConnectors: [],
      dependencies: [],
      requiredCapabilities: [],
      resolverStatus: "available",
      resolverErrors: [],
      permissionDiff: {},
      revisionId: "draft-new",
      contentHash: "hash",
      snapshotGeneration: 3
    })
  ]);
}

type MockRoute = {
  method: string;
  path: string;
  response: unknown | Error;
};

function route(method: string, path: string, response: unknown | Error): MockRoute {
  return { method, path, response };
}

function mockOwnerRequests(
  policy: OwnerPolicyFixture,
  routes: MockRoute[]
): {
  ownerPolicy: ReturnType<typeof vi.fn>;
  ownerRequest: ReturnType<typeof vi.fn>;
} {
  const bridge = {
    ownerPolicy: vi.fn().mockResolvedValue(policy),
    ownerRequest: vi.fn().mockImplementation((path: string, init: RequestInit) => {
      const match = routes.find(
        (candidate) => candidate.path === path && candidate.method === init.method
      );
      if (!match) {
        return Promise.reject(
          new Error(`Unexpected owner request: ${init.method ?? "GET"} ${path}`)
        );
      }
      return match.response instanceof Error
        ? Promise.reject(match.response)
        : Promise.resolve(match.response);
    })
  };
  Object.defineProperty(window, "generalAgent", {
    configurable: true,
    value: bridge
  });
  return bridge;
}

function managedInventoryFixture(options?: { draft?: boolean }): unknown {
  return {
    effective: [
      {
        package_id: "com.example.calendar",
        display_name: "Calendar Operations",
        version: "2.0.0",
        source_layer: "managed",
        status: options?.draft ? "draft" : "active",
        reason: "",
        active_revision_id: "draft-active",
        kind: "instruction_only",
        instructions: "# Calendar\n\nUse calendar tools.",
        validation: { ok: !options?.draft, errors: [], warnings: [] },
        requirements: {
          runtime_tools: ["calendar.read"],
          capabilities: [],
          connectors: [],
          packages: []
        },
        revisions: [
          {
            revision_id: "draft-active",
            version: "2.0.0",
            status: options?.draft ? "draft" : "active",
            created_by: "owner-1",
            created_at: "2026-07-12T10:00:00Z",
            kind: "instruction_only",
            instructions: "# Calendar\n\nUse calendar tools.",
            validation: { ok: !options?.draft, errors: [], warnings: [] },
            required_tools: ["calendar.read"],
            required_capabilities: [],
            permission_diff: { capabilities: { added: [] } }
          },
          {
            revision_id: "revision-1",
            version: "1.0.0",
            status: "managed",
            created_by: "owner-1",
            created_at: "2026-07-11T10:00:00Z",
            kind: "instruction_only",
            instructions: "# Calendar",
            validation: { ok: true, errors: [], warnings: [] },
            required_tools: [],
            required_capabilities: [],
            permission_diff: {}
          }
        ]
      }
    ],
    managed: []
  };
}

function mockOwnerRequest(path: string, payload: unknown): void {
  const bridge = window.generalAgent as unknown as {
    ownerRequest: ReturnType<typeof vi.fn>;
  };
  bridge.ownerRequest.mockImplementation((requestPath: string) => {
    if (requestPath === path) {
      return Promise.resolve(payload);
    }
    return Promise.reject(new Error(`Unexpected owner request: ${requestPath}`));
  });
}

function ownerInventoryFixture(): unknown {
  return {
    effective: [
      {
        package_id: "com.example.calendar",
        version: "2.0.0",
        source_layer: "managed",
        status: "active",
        reason: "",
        active_revision_id: "22222222-2222-4222-8222-222222222222"
      }
    ],
    managed: []
  };
}
