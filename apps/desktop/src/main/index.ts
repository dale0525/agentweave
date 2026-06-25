export type DesktopWindowConfig = {
  height: number;
  minHeight: number;
  minWidth: number;
  preload: string;
  title: string;
  width: number;
};

export const desktopWindowConfig: DesktopWindowConfig = {
  height: 900,
  minHeight: 720,
  minWidth: 1024,
  // Electron hosts should point BrowserWindow at the compiled preload bundle.
  preload: "dist/preload/index.js",
  title: "GeneralAgent",
  width: 1280
};

export function getDesktopWindowConfig(): DesktopWindowConfig {
  return desktopWindowConfig;
}
