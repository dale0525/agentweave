import { contextBridge } from "electron";

type DesktopRuntimeInfo = {
  platform: string;
  shell: "generalagent-desktop";
};

export type DesktopPreloadApi = {
  getRuntimeInfo: () => DesktopRuntimeInfo;
  ownerPolicy: () => Promise<unknown>;
  ownerRequest: (path: string, init: RequestInit) => Promise<unknown>;
};

const OWNER_SERVER_ORIGIN = "http://127.0.0.1:49321";
const ownerToken = process.env.GENERAL_AGENT_OWNER_TOKEN ?? "";
const configuredOwnerActorId = process.env.GENERAL_AGENT_OWNER_ACTOR_ID;
const configuredOwnerGrants = process.env.GENERAL_AGENT_OWNER_GRANTS;

const runtimeInfo: DesktopRuntimeInfo = {
  platform: typeof process === "undefined" ? "browser" : process.platform,
  shell: "generalagent-desktop"
};

export const desktopPreloadApi: DesktopPreloadApi = Object.freeze({
  getRuntimeInfo: () => runtimeInfo,
  ownerPolicy: async () => {
    const policy = await requestOwnerJson("/owner/policy", { method: "GET" });
    const mode = isRecord(policy) && typeof policy.mode === "string"
      ? policy.mode
      : "disabled";
    return {
      ...(isRecord(policy) ? policy : {}),
      actorId: configuredOwnerActorId ?? (mode === "owner_only" ? "local-owner" : "anonymous"),
      grants: configuredOwnerGrants
        ? splitGrants(configuredOwnerGrants)
        : mode === "owner_only"
          ? splitGrants(
              "inspect,create_draft,edit_draft,validate,test,activate,import,export,rollback,disable,delete_managed"
            )
          : mode === "disabled"
            ? []
            : ["inspect"]
    };
  },
  ownerRequest: (path, init) => requestOwnerJson(path, init)
});

if (typeof process !== "undefined" && process.contextIsolated) {
  contextBridge.exposeInMainWorld("generalAgent", desktopPreloadApi);
}

export function getDesktopRuntimeInfo(): DesktopRuntimeInfo {
  return desktopPreloadApi.getRuntimeInfo();
}

async function requestOwnerJson(path: string, init: RequestInit): Promise<unknown> {
  if (!ownerToken) {
    throw new Error("Owner skill management is disabled");
  }
  if (!path.startsWith("/owner/")) {
    throw new Error("Owner request path is not allowed");
  }

  const response = await fetch(`${OWNER_SERVER_ORIGIN}${path}`, {
    ...init,
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${ownerToken}`,
      ...init.headers
    }
  });
  const payload = await readPayload(response);
  if (!response.ok) {
    throw new Error(getErrorMessage(payload, response));
  }
  return payload;
}

async function readPayload(response: Response): Promise<unknown> {
  const text = await response.text();
  if (!text) {
    return {};
  }
  try {
    return JSON.parse(text);
  } catch {
    return { error: text };
  }
}

function getErrorMessage(payload: unknown, response: Response): string {
  if (isRecord(payload) && typeof payload.error === "string") {
    return payload.error;
  }
  return response.statusText || `HTTP ${response.status}`;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function splitGrants(value: string): string[] {
  return value.split(",").map((grant) => grant.trim()).filter(Boolean);
}
