const OWNER_SERVER_ORIGIN = "http://127.0.0.1:49321";

type ApprovalDecision = "approve" | "reject";

export function createApprovalTransport(
  approverToken: string,
  fetcher: typeof fetch = fetch
) {
  const request = (path: string, method: "GET" | "POST", body?: unknown) =>
    requestApprovalJson(fetcher, approverToken, path, method, body);
  return Object.freeze({
    principal: () => request("/owner/principal", "GET"),
    approval: (approvalId: string) =>
      request(`/owner/skills/approvals/${approvalUuid(approvalId)}`, "GET"),
    resolve: (approvalId: string, decision: ApprovalDecision) =>
      request(`/owner/skills/approvals/${approvalUuid(approvalId)}`, "POST", { decision })
  });
}

async function requestApprovalJson(
  fetcher: typeof fetch,
  token: string,
  path: string,
  method: "GET" | "POST",
  body?: unknown
): Promise<unknown> {
  if (!token) throw new Error("Independent approver credential is not configured");
  const url = new URL(path, OWNER_SERVER_ORIGIN);
  const allowed = url.origin === OWNER_SERVER_ORIGIN
    && !url.username
    && !url.password
    && !url.hash
    && (url.pathname === "/owner/principal"
      || /^\/owner\/skills\/approvals\/[0-9a-f-]+$/.test(url.pathname));
  if (!allowed || (url.pathname === "/owner/principal" && method !== "GET")) {
    throw new Error("Approval request path is not allowed");
  }
  const headers: Record<string, string> = { Authorization: `Bearer ${token}` };
  if (body !== undefined) headers["Content-Type"] = "application/json";
  const response = await fetcher(url.href, {
    body: body === undefined ? undefined : JSON.stringify(body),
    credentials: "omit",
    headers,
    method,
    redirect: "error"
  });
  const text = await response.text();
  const payload = text ? parsePayload(text) : {};
  if (!response.ok) {
    throw new Error(
      isRecord(payload) && typeof payload.error === "string"
        ? payload.error
        : response.statusText || `HTTP ${response.status}`
    );
  }
  return payload;
}

function approvalUuid(value: string): string {
  if (!/^[0-9a-f-]+$/.test(value)) throw new Error("Approval identifier is not allowed");
  return value;
}

function parsePayload(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return { error: text };
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}
