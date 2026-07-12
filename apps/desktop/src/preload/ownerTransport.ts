const OWNER_SERVER_ORIGIN = "http://127.0.0.1:49321";

type OwnerMethod = "DELETE" | "GET" | "POST" | "PUT";
type ApprovalDecision = "approve" | "reject";

type OwnerTransportOptions = {
  requesterToken: string;
  approverToken: string;
  fetcher?: typeof fetch;
};

const OWNER_ROUTES: Array<{ method: OwnerMethod; path: RegExp }> = [
  { method: "GET", path: /^\/owner\/principal$/ },
  { method: "GET", path: /^\/owner\/skills$/ },
  { method: "GET", path: /^\/owner\/skills\/[A-Za-z0-9._-]+\/detail$/ },
  { method: "POST", path: /^\/owner\/skills\/drafts$/ },
  { method: "PUT", path: /^\/owner\/skills\/drafts\/[0-9a-f-]+$/ },
  { method: "POST", path: /^\/owner\/skills\/drafts\/[0-9a-f-]+\/(validate|activation)$/ },
  { method: "POST", path: /^\/owner\/skills\/approvals\/[0-9a-f-]+$/ },
  { method: "POST", path: /^\/owner\/skills\/[A-Za-z0-9._-]+\/(rollback|disable)$/ },
  { method: "DELETE", path: /^\/owner\/skills\/[A-Za-z0-9._-]+$/ }
];

export function normalizeOwnerRequest(path: string, method: string): URL {
  if (!path.startsWith("/") || path.startsWith("//") || path.includes("\\")) {
    throw new Error("Owner request path is not allowed");
  }
  const url = new URL(path, OWNER_SERVER_ORIGIN);
  if (
    url.origin !== OWNER_SERVER_ORIGIN
    || url.username
    || url.password
    || url.hash
    || !url.pathname.startsWith("/owner/")
  ) {
    throw new Error("Owner request path is not allowed");
  }
  const normalizedMethod = method.toUpperCase() as OwnerMethod;
  const matchingPath = OWNER_ROUTES.filter((route) => route.path.test(url.pathname));
  if (matchingPath.length === 0) {
    throw new Error("Owner request path is not allowed");
  }
  if (!matchingPath.some((route) => route.method === normalizedMethod)) {
    throw new Error("Owner request method is not allowed");
  }
  return url;
}

export function createOwnerTransport({
  requesterToken,
  approverToken,
  fetcher = fetch
}: OwnerTransportOptions) {
  const requester = (path: string, method: OwnerMethod, body?: unknown) =>
    requestOwnerJson(fetcher, requesterToken, path, method, body);
  const approver = async (path: string, method: OwnerMethod, body?: unknown) => {
    if (!approverToken) {
      throw new Error("Independent approver credential is not configured");
    }
    return requestOwnerJson(fetcher, approverToken, path, method, body);
  };

  return Object.freeze({
    principal: () => requester("/owner/principal", "GET"),
    approverPrincipal: () => approver("/owner/principal", "GET"),
    listSkills: () => requester("/owner/skills", "GET"),
    skillDetail: (packageId: string) =>
      requester(`/owner/skills/${ownerPackageId(packageId)}/detail`, "GET"),
    createDraft: (request: unknown) => requester("/owner/skills/drafts", "POST", request),
    updateDraft: (revisionId: string, files: unknown) =>
      requester(`/owner/skills/drafts/${ownerUuid(revisionId)}`, "PUT", { files }),
    validateDraft: (revisionId: string) =>
      requester(`/owner/skills/drafts/${ownerUuid(revisionId)}/validate`, "POST", {}),
    requestActivation: (revisionId: string) =>
      requester(`/owner/skills/drafts/${ownerUuid(revisionId)}/activation`, "POST", {}),
    resolveApproval: (approvalId: string, decision: ApprovalDecision) =>
      approver(`/owner/skills/approvals/${ownerUuid(approvalId)}`, "POST", { decision }),
    rollback: (packageId: string, revisionId: string) =>
      requester(`/owner/skills/${ownerPackageId(packageId)}/rollback`, "POST", {
        revision_id: revisionId
      }),
    disable: (packageId: string) =>
      requester(`/owner/skills/${ownerPackageId(packageId)}/disable`, "POST", {}),
    requestRemoval: (packageId: string) =>
      requester(`/owner/skills/${ownerPackageId(packageId)}`, "DELETE")
  });
}

async function requestOwnerJson(
  fetcher: typeof fetch,
  token: string,
  path: string,
  method: OwnerMethod,
  body?: unknown
): Promise<unknown> {
  if (!token) throw new Error("Owner skill management is disabled");
  const url = normalizeOwnerRequest(path, method);
  const headers: Record<string, string> = { Authorization: `Bearer ${token}` };
  if (body !== undefined) headers["Content-Type"] = "application/json";
  const response = await fetcher(url.href, {
    body: body === undefined ? undefined : JSON.stringify(body),
    credentials: "omit",
    headers,
    method,
    redirect: "error"
  });
  const payload = await readPayload(response);
  if (!response.ok) throw new Error(getErrorMessage(payload, response));
  return payload;
}

function ownerPackageId(value: string): string {
  if (!/^[A-Za-z0-9._-]+$/.test(value)) throw new Error("Owner package id is not allowed");
  return value;
}

function ownerUuid(value: string): string {
  if (!/^[0-9a-f-]+$/.test(value)) throw new Error("Owner identifier is not allowed");
  return value;
}

async function readPayload(response: Response): Promise<unknown> {
  const text = await response.text();
  if (!text) return {};
  try {
    return JSON.parse(text);
  } catch {
    return { error: text };
  }
}

function getErrorMessage(payload: unknown, response: Response): string {
  return isRecord(payload) && typeof payload.error === "string"
    ? payload.error
    : response.statusText || `HTTP ${response.status}`;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}
