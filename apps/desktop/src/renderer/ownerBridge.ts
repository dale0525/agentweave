import type { ApprovalObservationResult } from "../shared/approvalObservation";

export type OwnerMode =
  | "disabled"
  | "diagnostics_only"
  | "owner_only"
  | "organization_managed";

export type OwnerPolicy = {
  mode: OwnerMode;
  actorId: string;
  role?: string;
  grants: string[];
  rollback_approval_required?: boolean;
};

type OwnerPrincipalWire = {
  actorId: string;
  role: string;
  grants: string[];
  policy: Omit<OwnerPolicy, "actorId" | "role" | "grants">;
};

export const disabledOwnerPolicy: OwnerPolicy = {
  mode: "disabled",
  actorId: "anonymous",
  role: "anonymous",
  grants: []
};

export async function getOwnerPolicy(): Promise<OwnerPolicy> {
  const principal = requirePrincipal(await getOwnerApi().principal());
  return principalPolicy(principal);
}

export function getOwnerApi(): NonNullable<Window["agentWeave"]>["owner"] {
  const useDevServer = import.meta.env.DEV
    && new URLSearchParams(window.location.search).get("ownerServer") === "1";
  const owner = window.agentWeave?.owner ?? (useDevServer ? devBrowserOwnerApi : null);
  if (!owner) throw new Error("Owner skill management is disabled");
  return owner;
}

const devBrowserOwnerApi: NonNullable<Window["agentWeave"]>["owner"] = {
  principal: () => devRequest("requester", "/owner/principal", "GET"),
  listSkills: () => devRequest("requester", "/owner/skills", "GET"),
  skillDetail: (packageId) => devRequest("requester", `/owner/skills/${packageId}/detail`, "GET"),
  createDraft: (request) => devRequest("requester", "/owner/skills/drafts", "POST", request),
  updateDraft: (revisionId, files) => devRequest("requester", `/owner/skills/drafts/${revisionId}`, "PUT", { files }),
  validateDraft: (revisionId) => devRequest("requester", `/owner/skills/drafts/${revisionId}/validate`, "POST", {}),
  requestActivation: (revisionId) => devRequest("requester", `/owner/skills/drafts/${revisionId}/activation`, "POST", {}),
  rollback: (packageId, revisionId) => devRequest("requester", `/owner/skills/${packageId}/rollback`, "POST", { revision_id: revisionId }),
  disable: (packageId) => devRequest("requester", `/owner/skills/${packageId}/disable`, "POST", {}),
  requestRemoval: (packageId) => devRequest("requester", `/owner/skills/${packageId}`, "DELETE")
};

export async function requestApprovalSurface(
  approvalId: string
): Promise<ApprovalObservationResult> {
  const approval = window.agentWeave?.approval;
  if (!approval) throw new Error("Independent approval surface is unavailable");
  return requireApprovalObservation(await approval.open(approvalId), approvalId);
}

function requireApprovalObservation(
  value: unknown,
  approvalId: string
): ApprovalObservationResult {
  if (!isRecord(value) || value.approvalId !== approvalId) {
    throw new Error("Approval observation result is invalid");
  }
  if (value.status === "completed") {
    if (value.decision !== "approve" && value.decision !== "reject") {
      throw new Error("Approval observation decision is invalid");
    }
    return {
      approvalId,
      decision: value.decision,
      ...(Object.hasOwn(value, "resolution") ? { resolution: value.resolution } : {}),
      status: "completed"
    };
  }
  if (value.status === "closed" || value.status === "disposed" || value.status === "load_failed") {
    return { approvalId, status: value.status };
  }
  throw new Error("Approval observation status is invalid");
}

async function devRequest(actor: "requester", path: string, method: string, body?: unknown): Promise<unknown> {
  const response = await fetch(`/__owner/${actor}${path}`, {
    body: body === undefined ? undefined : JSON.stringify(body),
    headers: body === undefined ? undefined : { "Content-Type": "application/json" },
    method
  });
  const payload = await response.json().catch(() => ({})) as { error?: string };
  if (!response.ok) throw new Error(payload.error ?? `HTTP ${response.status}`);
  return payload;
}

export function canInspectOwnerSkills(policy: OwnerPolicy | null): boolean {
  return Boolean(
    policy
    && policy.mode !== "disabled"
    && (policy.mode !== "owner_only" || policy.role === "owner")
    && policy.grants?.includes("inspect")
  );
}

export function canManageOwnerSkills(policy: OwnerPolicy, grant: string): boolean {
  return policy.mode === "owner_only" && policy.role === "owner" && policy.grants.includes(grant);
}

function principalPolicy(principal: OwnerPrincipalWire): OwnerPolicy {
  return {
    ...principal.policy,
    actorId: principal.actorId,
    role: principal.role,
    grants: principal.grants
  };
}

function requirePrincipal(value: unknown): OwnerPrincipalWire {
  if (
    typeof value !== "object"
    || value === null
    || !("actorId" in value)
    || typeof value.actorId !== "string"
    || !("role" in value)
    || typeof value.role !== "string"
    || !("grants" in value)
    || !Array.isArray(value.grants)
    || !value.grants.every((grant) => typeof grant === "string")
    || !("policy" in value)
    || typeof value.policy !== "object"
    || value.policy === null
    || !("mode" in value.policy)
    || typeof value.policy.mode !== "string"
  ) {
    throw new Error("Authenticated owner principal response is invalid");
  }
  return value as OwnerPrincipalWire;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}
