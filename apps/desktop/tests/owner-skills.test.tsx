import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import App from "../src/renderer/App";
import { OwnerSkillPackage } from "../src/renderer/api";
import { OwnerPolicy } from "../src/renderer/ownerBridge";
import { OwnerSkills } from "../src/renderer/screens/OwnerSkills";
import detailFixture from "./fixtures/owner-package-detail.json";

class TestResizeObserver implements ResizeObserver {
  disconnect(): void {}
  observe(): void {}
  unobserve(): void {}
}

const ownerPolicy: OwnerPolicy = {
  mode: "owner_only",
  actorId: "owner-1",
  role: "owner",
  grants: ["inspect", "create_draft", "edit_draft", "validate", "activate", "rollback", "disable", "delete_managed"]
};
const approverPolicy: OwnerPolicy = { ...ownerPolicy, actorId: "approver-2" };
const inventory = {
  effective: [{
    package_id: "com.example.calendar", display_name: "Calendar Operations", version: "2.0.0", source_layer: "managed",
    status: "active", reason: "active", active_revision_id: "22222222-2222-4222-8222-222222222222"
  }],
  managed: []
};

beforeEach(() => {
  vi.stubGlobal("ResizeObserver", TestResizeObserver);
  Object.defineProperty(window, "innerWidth", { configurable: true, value: 1280 });
});

afterEach(() => {
  cleanup();
  window.history.replaceState(null, "", "/");
  delete window.generalAgent;
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("owner principal route gating", () => {
  it("hides settings and fails closed on direct route without inspect", async () => {
    window.history.replaceState(null, "", "/#owner-skills");
    installBridge({ policy: { ...ownerPolicy, grants: [] } });
    render(<App />);
    expect(screen.getByRole("main", { name: "Settings" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Manage skills" })).not.toBeInTheDocument();
    await waitFor(() => expect(window.location.hash).toBe("#settings"));
  });

  it("uses the authenticated principal for an authorized owner route", async () => {
    installBridge();
    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "Open settings" }));
    await userEvent.click(await screen.findByRole("button", { name: "Manage skills" }));
    expect(await screen.findByRole("main", { name: "Owner Skills" })).toBeInTheDocument();
    expect(await screen.findAllByText("Calendar Operations")).not.toHaveLength(0);
  });
});

describe("owner workflows", () => {
  it("uses distinct requester and approver actors and reloads after approval", async () => {
    const bridge = installBridge();
    renderOwner();
    await userEvent.click(await screen.findByRole("tab", { name: "Draft" }));
    await userEvent.click(screen.getByRole("button", { name: "Request activation" }));
    const dialog = await screen.findByRole("dialog", { name: "Approve skill activation" });
    expect(within(dialog).getByText("owner-1")).toBeInTheDocument();
    expect(within(dialog).getByText("approver-2")).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("button", { name: "Approve activation" }));
    expect(await screen.findByText("Active snapshot 4")).toBeInTheDocument();
    expect(bridge.resolveApproval).toHaveBeenCalledWith("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa", "approve");
    expect(bridge.listSkills.mock.calls.length).toBeGreaterThan(1);
    expect(bridge.skillDetail.mock.calls.length).toBeGreaterThan(1);
  });

  it("disables approval with a clear state when no independent approver exists", async () => {
    installBridge({ approverError: new Error("Independent approver credential is not configured") });
    renderOwner();
    await userEvent.click(await screen.findByRole("tab", { name: "Draft" }));
    await userEvent.click(screen.getByRole("button", { name: "Request activation" }));
    const dialog = await screen.findByRole("dialog", { name: "Approve skill activation" });
    expect(within(dialog).getByText("Independent approver credential is not configured")).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Approve activation" })).toBeDisabled();
  });

  it("does not expose a draft editor for an active package without editable_draft", async () => {
    const activeOnly = structuredClone(detailFixture) as OwnerSkillPackage;
    activeOnly.revisions = activeOnly.revisions.filter((revision) => !revision.editable);
    activeOnly.editable_draft = null;
    installBridge({ detail: activeOnly });
    renderOwner();
    expect(await screen.findAllByText("Calendar Operations")).not.toHaveLength(0);
    expect(screen.queryByRole("tab", { name: "Draft" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Request activation" })).not.toBeInTheDocument();
  });

  it("invalidates a passing validation immediately after an edit", async () => {
    installBridge();
    renderOwner();
    await userEvent.click(await screen.findByRole("tab", { name: "Draft" }));
    expect(screen.getByRole("button", { name: "Request activation" })).toBeEnabled();
    await userEvent.type(screen.getByRole("textbox", { name: "Instructions" }), " changed");
    expect(screen.getByRole("button", { name: "Request activation" })).toBeDisabled();
    expect(screen.getByText("Validation has not run")).toBeInTheDocument();
  });

  it("never offers rollback to an editable staging revision", async () => {
    installBridge();
    renderOwner();
    expect(await screen.findAllByText("Calendar Operations")).not.toHaveLength(0);
    expect(screen.queryByRole("button", { name: "Rollback to 2.1.0" })).not.toBeInTheDocument();
  });

  it("merges authoritative validation requirements into approval", async () => {
    installBridge({ validation: {
      ok: true, errors: [], warnings: [], requiredTools: ["calendar.write"],
      requiredCapabilities: ["calendar.modify"], requiredConnectors: ["cloud.calendar"],
      dependencies: ["com.example.base"], permissionDiff: { capabilities: { added: ["calendar.modify"] } }
    }});
    renderOwner();
    await userEvent.click(await screen.findByRole("tab", { name: "Draft" }));
    await userEvent.click(screen.getByRole("button", { name: "Validate draft" }));
    await userEvent.click(await screen.findByRole("button", { name: "Request activation" }));
    const dialog = await screen.findByRole("dialog", { name: "Approve skill activation" });
    expect(within(dialog).getByText("calendar.write")).toBeInTheDocument();
    expect(within(dialog).getByText("calendar.modify")).toBeInTheDocument();
    expect(within(dialog).getByText("cloud.calendar")).toBeInTheDocument();
    expect(within(dialog).getByText("com.example.base")).toBeInTheDocument();
  });

  it("persists current editor bytes before validation and stops on save failure", async () => {
    const bridge = installBridge();
    bridge.updateDraft.mockRejectedValueOnce(new Error("Draft content is invalid"));
    renderOwner();
    await userEvent.click(await screen.findByRole("tab", { name: "Draft" }));
    await userEvent.type(screen.getByRole("textbox", { name: "Instructions" }), " unsaved");
    await userEvent.click(screen.getByRole("button", { name: "Validate draft" }));
    expect(await screen.findByText("Draft content is invalid")).toBeInTheDocument();
    expect(bridge.updateDraft).toHaveBeenCalled();
    expect(bridge.validateDraft).not.toHaveBeenCalled();
    expect((screen.getByRole("textbox", { name: "Instructions" }) as HTMLTextAreaElement).value).toContain("unsaved");
  });

  it("retains current editor content after authoritative validation errors", async () => {
    installBridge({ validation: { ok: false, errors: ["SKILL.md must start with front matter"], warnings: [] } });
    renderOwner();
    await userEvent.click(await screen.findByRole("tab", { name: "Draft" }));
    const editor = screen.getByRole("textbox", { name: "Instructions" });
    await userEvent.clear(editor);
    await userEvent.type(editor, "No front matter");
    await userEvent.click(screen.getByRole("button", { name: "Validate draft" }));
    expect(await screen.findByText("SKILL.md must start with front matter")).toBeInTheDocument();
    expect(editor).toHaveValue("No front matter");
    expect(screen.getByRole("button", { name: "Request activation" })).toBeDisabled();
  });

  it("keeps inventory visible and surfaces refresh failure", async () => {
    const bridge = installBridge();
    renderOwner();
    expect(await screen.findAllByText("Calendar Operations")).not.toHaveLength(0);
    bridge.listSkills.mockRejectedValueOnce(new Error("refresh unavailable"));
    await userEvent.click(screen.getByRole("button", { name: "Refresh skills" }));
    expect(await screen.findByText("refresh unavailable")).toBeInTheDocument();
    expect(screen.getAllByText("Calendar Operations")).not.toHaveLength(0);
  });

  it("keeps the approval dialog open and visible after approval failure", async () => {
    const bridge = installBridge();
    bridge.resolveApproval.mockRejectedValueOnce(new Error("approver service unavailable"));
    renderOwner();
    await userEvent.click(await screen.findByRole("tab", { name: "Draft" }));
    await userEvent.click(screen.getByRole("button", { name: "Request activation" }));
    const dialog = await screen.findByRole("dialog", { name: "Approve skill activation" });
    await userEvent.click(within(dialog).getByRole("button", { name: "Approve activation" }));
    expect(await within(dialog).findByText("approver service unavailable")).toBeInTheDocument();
    expect(dialog).toBeInTheDocument();
  });

  it("resolves approval-required rollback with the approver and reloads", async () => {
    const rollbackDetail = structuredClone(detailFixture) as OwnerSkillPackage;
    rollbackDetail.editable_draft = null;
    rollbackDetail.revisions = rollbackDetail.revisions.filter((revision) => !revision.editable);
    rollbackDetail.revisions.push({
      ...structuredClone(rollbackDetail.revisions[0]),
      revision_id: "11111111-1111-4111-8111-111111111111",
      version: "1.0.0"
    });
    const bridge = installBridge({ detail: rollbackDetail });
    bridge.rollback.mockResolvedValueOnce({
      approval_id: "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
      package_id: "com.example.calendar",
      permission_diff: {}, requested_by: "owner-1",
      revision_id: "11111111-1111-4111-8111-111111111111", status: "pending"
    });
    renderOwner();
    await userEvent.click(await screen.findByRole("button", { name: "Rollback to 1.0.0" }));
    const dialog = await screen.findByRole("dialog", { name: "Approve skill rollback" });
    expect(within(dialog).getByText("approver-2")).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("button", { name: "Approve rollback" }));
    expect(bridge.resolveApproval).toHaveBeenCalledWith("cccccccc-cccc-4ccc-8ccc-cccccccccccc", "approve");
    expect(bridge.listSkills.mock.calls.length).toBeGreaterThan(1);
  });

  it("routes removal approval through the independent approver", async () => {
    const activeOnly = structuredClone(detailFixture) as OwnerSkillPackage;
    activeOnly.editable_draft = null;
    activeOnly.revisions = activeOnly.revisions.filter((revision) => !revision.editable);
    const bridge = installBridge({ detail: activeOnly });
    renderOwner();
    await userEvent.click(await screen.findByRole("button", { name: "Remove skill" }));
    const dialog = await screen.findByRole("dialog", { name: "Approve skill removal" });
    await userEvent.click(within(dialog).getByRole("button", { name: "Approve removal" }));
    expect(bridge.requestRemoval).toHaveBeenCalledWith("com.example.calendar");
    expect(bridge.resolveApproval).toHaveBeenCalledWith("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb", "approve");
  });

  it("hides inapplicable lifecycle actions for removed packages", async () => {
    const removed = structuredClone(detailFixture) as OwnerSkillPackage;
    removed.status = "removed";
    removed.editable_draft = null;
    removed.revisions = removed.revisions.filter((revision) => !revision.editable);
    installBridge({ detail: removed });
    renderOwner();
    expect(await screen.findAllByText("Calendar Operations")).not.toHaveLength(0);
    expect(screen.queryByRole("button", { name: "Disable skill" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Remove skill" })).not.toBeInTheDocument();
  });

  it("synchronizes the editor when refreshing the same selected package", async () => {
    const bridge = installBridge();
    let current = structuredClone(detailFixture) as OwnerSkillPackage;
    bridge.skillDetail.mockImplementation(async () => structuredClone(current));
    renderOwner();
    await userEvent.click(await screen.findByRole("tab", { name: "Draft" }));
    expect(screen.getByRole("textbox", { name: "Instructions" })).toHaveValue(current.editable_draft?.instructions);
    current = structuredClone(current);
    current.editable_draft!.instructions = "# Refreshed authoritative draft";
    current.revisions[0].instructions = "# Refreshed authoritative draft";
    await userEvent.click(screen.getByRole("button", { name: "Refresh skills" }));
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Instructions" })).toHaveValue("# Refreshed authoritative draft"));
  });

  it("ignores an older inventory response that resolves last", async () => {
    let resolveOld!: (value: typeof inventory) => void;
    const old = new Promise<typeof inventory>((resolve) => { resolveOld = resolve; });
    const newer = { effective: [{ ...inventory.effective[0], package_id: "com.example.newer" }], managed: [] };
    const bridge = installBridge();
    bridge.listSkills.mockReset().mockReturnValueOnce(old).mockResolvedValueOnce(newer);
    bridge.skillDetail.mockImplementation(async (id: string) => ({ ...detailFixture, package_id: id, display_name: id }));
    renderOwner();
    await userEvent.click(screen.getByRole("button", { name: "Refresh skills" }));
    expect(await screen.findAllByText("com.example.newer")).not.toHaveLength(0);
    resolveOld(inventory);
    await waitFor(() => expect(screen.queryByText("com.example.calendar")).not.toBeInTheDocument());
  });

  it("keeps package controls as buttons and renders mobile revisions as vertical records", async () => {
    Object.defineProperty(window, "innerWidth", { configurable: true, value: 390 });
    installBridge();
    renderOwner();
    const list = await screen.findByRole("list", { name: "Skill packages" });
    const packageButton = within(list).getByRole("button");
    await userEvent.click(packageButton);
    await userEvent.click(await screen.findByRole("tab", { name: "Revisions" }));
    expect(screen.queryByRole("table", { name: "Revision history" })).not.toBeInTheDocument();
    expect(screen.getByRole("list", { name: "Revision history" })).toBeInTheDocument();
  });
});

function renderOwner() {
  return render(<OwnerSkills onBack={() => undefined} policy={ownerPolicy} />);
}

function installBridge(options: {
  policy?: OwnerPolicy;
  approverError?: Error;
  detail?: OwnerSkillPackage;
  validation?: Record<string, unknown>;
} = {}) {
  const requester = principal(options.policy ?? ownerPolicy);
  const api = {
    principal: vi.fn(async () => requester),
    approverPrincipal: options.approverError
      ? vi.fn(async () => { throw options.approverError; })
      : vi.fn(async () => principal(approverPolicy)),
    listSkills: vi.fn(async () => inventory),
    skillDetail: vi.fn(async (_id: string) => options.detail ?? structuredClone(detailFixture)),
    createDraft: vi.fn(),
    updateDraft: vi.fn(async () => ({})),
    validateDraft: vi.fn(async () => options.validation ?? { ok: true, errors: [], warnings: [] }),
    requestActivation: vi.fn(async () => ({
      approval_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa", package_id: "com.example.calendar",
      permission_diff: {}, requested_by: "owner-1", revision_id: "33333333-3333-4333-8333-333333333333", status: "pending"
    })),
    resolveApproval: vi.fn(async () => ({ status: "approved", active_generation: 4 })),
    rollback: vi.fn(async (): Promise<Record<string, unknown>> => ({ generation: 5 })),
    disable: vi.fn(async () => ({})),
    requestRemoval: vi.fn(async () => ({
      approval_id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb", package_id: "com.example.calendar",
      permission_diff: {}, requested_by: "owner-1", revision_id: "22222222-2222-4222-8222-222222222222", status: "pending"
    }))
  };
  window.generalAgent = { owner: api };
  return api;
}

function principal(policy: OwnerPolicy) {
  const { actorId, role, grants, ...policyFields } = policy;
  return { actorId, role: role ?? "owner", grants, policy: policyFields };
}
