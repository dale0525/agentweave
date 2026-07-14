import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ApprovalSurface } from "../src/renderer/screens/ApprovalSurface";

const APPROVAL_ID = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

describe("approval surface", () => {
  afterEach(cleanup);

  beforeEach(() => {
    window.history.replaceState({}, "", `/approval.html?approvalId=${APPROVAL_ID}`);
  });

  it("loads trusted review context and requires an explicit decision", async () => {
    const resolve = vi.fn(async () => ({ status: "approved" }));
    const complete = vi.fn(async () => ({}));
    window.agentWeaveApproval = {
      principal: vi.fn(async () => ({ actorId: "approver-2", role: "owner" })),
      approval: vi.fn(async () => approvalReview()),
      resolve,
      complete,
      close: vi.fn(async () => ({}))
    };
    const user = userEvent.setup();

    render(<ApprovalSurface />);

    expect(await screen.findByRole("heading", { name: "Review skill activation" })).toBeVisible();
    expect(screen.getByText("com.example.calendar")).toBeVisible();
    expect(screen.getByText("approver-2")).toBeVisible();
    expect(resolve).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "Approve" }));

    await waitFor(() => expect(resolve).toHaveBeenCalledWith(APPROVAL_ID, "approve"));
    expect(complete).toHaveBeenCalledWith({
      approvalId: APPROVAL_ID,
      decision: "approve",
      resolution: { status: "approved" }
    });
  });

  it("supports explicit rejection without exposing requester APIs", async () => {
    const resolve = vi.fn(async () => ({ status: "rejected" }));
    window.agentWeaveApproval = {
      principal: vi.fn(async () => ({ actorId: "approver-2", role: "owner" })),
      approval: vi.fn(async () => approvalReview()),
      resolve,
      complete: vi.fn(async () => ({})),
      close: vi.fn(async () => ({}))
    };
    const user = userEvent.setup();

    render(<ApprovalSurface />);
    await user.click(await screen.findByRole("button", { name: "Reject" }));

    await waitFor(() => expect(resolve).toHaveBeenCalledWith(APPROVAL_ID, "reject"));
    expect("owner" in window.agentWeaveApproval).toBe(false);
  });

  it("lets the approver close observation without making a decision", async () => {
    const resolve = vi.fn(async () => ({ status: "approved" }));
    const close = vi.fn(async () => ({}));
    window.agentWeaveApproval = {
      principal: vi.fn(async () => ({ actorId: "approver-2", role: "owner" })),
      approval: vi.fn(async () => approvalReview()),
      resolve,
      complete: vi.fn(async () => ({})),
      close
    };
    const user = userEvent.setup();

    render(<ApprovalSurface />);
    await user.click(await screen.findByRole("button", { name: "Close approval window" }));

    expect(close).toHaveBeenCalledWith(APPROVAL_ID);
    expect(resolve).not.toHaveBeenCalled();
  });
});

function approvalReview() {
  return {
    approval: {
      approval_id: APPROVAL_ID,
      operation: "activation",
      package_id: "com.example.calendar",
      permission_diff: { capabilities: { added: ["calendar.write"] } },
      requested_by: "requester-1",
      revision_id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
      status: "pending"
    },
    package: {
      active_revision_id: "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
      display_name: "Calendar",
      package_id: "com.example.calendar",
      revisions: [
        {
          created_at: "2026-07-13T00:00:00Z",
          created_by: "requester-1",
          editable: false,
          instructions: "Use calendar safely.",
          kind: "instruction_only",
          requirements: {
            capabilities: ["calendar.write"],
            connectors: ["calendar"],
            packages: [],
            runtime_tools: ["calendar.create"]
          },
          revision_id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
          status: "staging",
          validation: { errors: [], ok: true, warnings: [] },
          version: "1.1.0"
        },
        {
          created_at: "2026-07-12T00:00:00Z",
          created_by: "requester-1",
          editable: false,
          instructions: "Use calendar.",
          kind: "instruction_only",
          requirements: { capabilities: [], connectors: [], packages: [], runtime_tools: [] },
          revision_id: "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
          status: "managed",
          validation: { errors: [], ok: true, warnings: [] },
          version: "1.0.0"
        }
      ]
    }
  };
}
