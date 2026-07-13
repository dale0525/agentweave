import { contextBridge, ipcRenderer } from "electron";
import { createApprovalTransport } from "./approvalTransport";

const transport = createApprovalTransport(
  process.env.GENERAL_AGENT_APPROVER_TOKEN ?? ""
);

const approvalApi = Object.freeze({
  ...transport,
  complete: (result: unknown) => ipcRenderer.invoke("general-agent:approval:complete", result)
});

if (typeof process !== "undefined" && process.contextIsolated) {
  contextBridge.exposeInMainWorld("generalAgentApproval", approvalApi);
}

export type DesktopApprovalPreloadApi = typeof approvalApi;
