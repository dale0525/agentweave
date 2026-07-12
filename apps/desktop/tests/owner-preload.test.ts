import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  createOwnerTransport,
  normalizeOwnerRequest
} from "../src/preload/ownerTransport";

beforeEach(() => {
  vi.restoreAllMocks();
});

describe("owner preload capability", () => {
  it("rejects traversal, non-owner paths, origins, and unsupported methods", () => {
    for (const path of [
      "/owner/../sessions",
      "/owner/%2e%2e/sessions",
      "/owner/skills/../../model/test",
      "/sessions",
      "http://attacker.invalid/owner/skills"
    ]) {
      expect(() => normalizeOwnerRequest(path, "GET")).toThrow(/not allowed/);
    }
    expect(() => normalizeOwnerRequest("/owner/skills", "PATCH")).toThrow(
      /method is not allowed/
    );
  });

  it("sets security headers itself and never accepts renderer headers", async () => {
    const fetcher = vi.fn(async () =>
      new Response(JSON.stringify({ effective: [], managed: [] }), { status: 200 })
    );
    const transport = createOwnerTransport({
      requesterToken: "requester-secret",
      approverToken: "approver-secret",
      fetcher
    });

    await transport.listSkills();

    expect(fetcher).toHaveBeenCalledWith(
      "http://127.0.0.1:49321/owner/skills",
      expect.objectContaining({
        method: "GET",
        credentials: "omit",
        headers: { Authorization: "Bearer requester-secret" }
      })
    );
  });

  it("uses the independent approver credential only for approval resolution", async () => {
    const fetcher = vi.fn(async () =>
      new Response(JSON.stringify({ status: "approved" }), { status: 200 })
    );
    const transport = createOwnerTransport({
      requesterToken: "requester-secret",
      approverToken: "approver-secret",
      fetcher
    });

    await transport.resolveApproval("00000000-0000-4000-8000-000000000001", "approve");

    expect(fetcher).toHaveBeenCalledWith(
      expect.stringContaining("/owner/skills/approvals/"),
      expect.objectContaining({
        headers: expect.objectContaining({ Authorization: "Bearer approver-secret" })
      })
    );
  });

  it("reports an explicit unavailable state without an independent approver", async () => {
    const transport = createOwnerTransport({
      requesterToken: "requester-secret",
      approverToken: "",
      fetcher: vi.fn()
    });

    await expect(
      transport.resolveApproval("00000000-0000-4000-8000-000000000001", "approve")
    ).rejects.toThrow("Independent approver credential is not configured");
  });
});
