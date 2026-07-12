import { contextBridge } from "electron";
import { createOwnerTransport } from "./ownerTransport";

type DesktopRuntimeInfo = {
  platform: string;
  shell: "generalagent-desktop";
};

export type DesktopPreloadApi = {
  getRuntimeInfo: () => DesktopRuntimeInfo;
  owner: ReturnType<typeof createOwnerTransport>;
};

const ownerToken = process.env.GENERAL_AGENT_OWNER_TOKEN ?? "";
const approverToken = process.env.GENERAL_AGENT_APPROVER_TOKEN ?? "";
const owner = createOwnerTransport({ requesterToken: ownerToken, approverToken });

const runtimeInfo: DesktopRuntimeInfo = {
  platform: typeof process === "undefined" ? "browser" : process.platform,
  shell: "generalagent-desktop"
};

export const desktopPreloadApi: DesktopPreloadApi = Object.freeze({
  getRuntimeInfo: () => runtimeInfo,
  owner
});

if (typeof process !== "undefined" && process.contextIsolated) {
  contextBridge.exposeInMainWorld("generalAgent", desktopPreloadApi);
}

export function getDesktopRuntimeInfo(): DesktopRuntimeInfo {
  return desktopPreloadApi.getRuntimeInfo();
}
