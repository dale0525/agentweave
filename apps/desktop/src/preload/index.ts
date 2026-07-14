import { contextBridge, ipcRenderer } from "electron";
import type { ApprovalObservationResult } from "../shared/approvalObservation";
import { createOwnerTransport } from "./ownerTransport";

type DesktopRuntimeInfo = {
  platform: string;
  shell: "generalagent-desktop";
};

export type DesktopPreloadApi = {
  getRuntimeInfo: () => DesktopRuntimeInfo;
  owner: ReturnType<typeof createOwnerTransport>;
  approval: {
    open: (approvalId: string) => Promise<ApprovalObservationResult>;
  };
  modelSettings: {
    clearApiKey: () => Promise<unknown>;
    load: () => Promise<unknown>;
    postSessionMessage: (sessionId: string, content: string) => Promise<unknown>;
    save: (settings: unknown) => Promise<unknown>;
    testConnection: () => Promise<unknown>;
  };
};

const ownerToken = process.env.GENERAL_AGENT_OWNER_TOKEN ?? "";
const owner = createOwnerTransport({ requesterToken: ownerToken });

const runtimeInfo: DesktopRuntimeInfo = {
  platform: typeof process === "undefined" ? "browser" : process.platform,
  shell: "generalagent-desktop"
};

export const desktopPreloadApi: DesktopPreloadApi = Object.freeze({
  getRuntimeInfo: () => runtimeInfo,
  owner,
  approval: Object.freeze({
    open: (approvalId: string): Promise<ApprovalObservationResult> => {
      if (!/^[0-9a-f-]+$/.test(approvalId)) {
        return Promise.reject(new Error("Approval identifier is not allowed"));
      }
      return ipcRenderer.invoke("general-agent:approval:open", approvalId) as Promise<ApprovalObservationResult>;
    }
  }),
  modelSettings: Object.freeze({
    clearApiKey: () => ipcRenderer.invoke("general-agent:model-settings:clear-key") as Promise<unknown>,
    load: () => ipcRenderer.invoke("general-agent:model-settings:load") as Promise<unknown>,
    postSessionMessage: (sessionId: string, content: string) =>
      ipcRenderer.invoke("general-agent:model-settings:message", { sessionId, content }) as Promise<unknown>,
    save: (settings: unknown) =>
      ipcRenderer.invoke("general-agent:model-settings:save", settings) as Promise<unknown>,
    testConnection: () => ipcRenderer.invoke("general-agent:model-settings:test") as Promise<unknown>
  })
});

if (typeof process !== "undefined" && process.contextIsolated) {
  contextBridge.exposeInMainWorld("generalAgent", desktopPreloadApi);
}

export function getDesktopRuntimeInfo(): DesktopRuntimeInfo {
  return desktopPreloadApi.getRuntimeInfo();
}
