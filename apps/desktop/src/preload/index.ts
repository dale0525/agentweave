type DesktopRuntimeInfo = {
  platform: string;
  shell: "generalagent-desktop";
};

declare global {
  interface Window {
    generalAgent?: DesktopRuntimeInfo;
  }
}

const runtimeInfo: DesktopRuntimeInfo = {
  platform: typeof process === "undefined" ? "browser" : process.platform,
  shell: "generalagent-desktop"
};

if (typeof window !== "undefined") {
  Object.defineProperty(window, "generalAgent", {
    configurable: false,
    enumerable: true,
    value: runtimeInfo
  });
}

export {};
