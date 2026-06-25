type DesktopRuntimeInfo = {
  platform: string;
  shell: "generalagent-desktop";
};

export type DesktopPreloadApi = {
  getRuntimeInfo: () => DesktopRuntimeInfo;
};

const runtimeInfo: DesktopRuntimeInfo = {
  platform: typeof process === "undefined" ? "browser" : process.platform,
  shell: "generalagent-desktop"
};

export const desktopPreloadApi: DesktopPreloadApi = Object.freeze({
  getRuntimeInfo: () => runtimeInfo
});

export function getDesktopRuntimeInfo(): DesktopRuntimeInfo {
  return desktopPreloadApi.getRuntimeInfo();
}
