import { app, BrowserWindow, ipcMain } from "electron";
import path from "node:path";
import { pathToFileURL } from "node:url";

import { registerApprovalWindowController } from "./approvalWindow";
import { getDesktopWindowConfig } from "./index";

let mainWindow: BrowserWindow | null = null;
let disposeApproval: (() => void) | null = null;

app.whenReady().then(async () => {
  const rendererBase = process.env.GENERAL_AGENT_DESKTOP_URL;
  mainWindow = new BrowserWindow({
    ...getDesktopWindowConfig(),
    webPreferences: {
      ...getDesktopWindowConfig().webPreferences,
      preload: path.join(__dirname, "preload.cjs")
    }
  });
  const mainUrl = rendererBase
    ? new URL("/", rendererBase).href
    : pathToFileURL(path.join(__dirname, "../dist/index.html")).href;
  const approvalUrl = rendererBase
    ? new URL("/approval.html", rendererBase).href
    : pathToFileURL(path.join(__dirname, "../dist/approval.html")).href;
  disposeApproval = registerApprovalWindowController({
    approvalPreload: path.join(__dirname, "approval-preload.cjs"),
    approvalUrl,
    createWindow: (options) => new BrowserWindow({ ...options, parent: mainWindow! }),
    ipcMain,
    requesterWebContents: mainWindow.webContents
  });
  await mainWindow.loadURL(mainUrl);
});

app.on("window-all-closed", () => {
  disposeApproval?.();
  disposeApproval = null;
  mainWindow = null;
  if (process.platform !== "darwin") app.quit();
});
