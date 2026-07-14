import { contextBridge, ipcRenderer } from "electron";
import { createApprovalTransport } from "./approvalTransport";

const transport = createApprovalTransport(
  process.env.AGENTWEAVE_APPROVER_TOKEN ?? ""
);

const approvalApi = Object.freeze({
  ...transport,
  complete: (result: unknown) => ipcRenderer.invoke("agentweave:approval:complete", result),
  close: (approvalId: string) => ipcRenderer.invoke("agentweave:approval:close", approvalId)
});

if (typeof process !== "undefined" && process.contextIsolated) {
  contextBridge.exposeInMainWorld("agentWeaveApproval", approvalApi);
}

export type DesktopApprovalPreloadApi = typeof approvalApi;
