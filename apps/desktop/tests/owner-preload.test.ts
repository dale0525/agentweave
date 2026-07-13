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
    const transport = createOwnerTransport({ requesterToken: "requester-secret", fetcher });

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

  it("does not expose approver identity or resolution capability to requester renderer", () => {
    const transport = createOwnerTransport({
      requesterToken: "requester-secret",
      fetcher: vi.fn()
    });

    expect("approverPrincipal" in transport).toBe(false);
    expect("resolveApproval" in transport).toBe(false);
    expect(Object.keys(transport)).not.toContain("approverToken");
  });
});
