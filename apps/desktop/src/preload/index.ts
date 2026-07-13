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
  })
});

if (typeof process !== "undefined" && process.contextIsolated) {
  contextBridge.exposeInMainWorld("generalAgent", desktopPreloadApi);
}

export function getDesktopRuntimeInfo(): DesktopRuntimeInfo {
  return desktopPreloadApi.getRuntimeInfo();
}
