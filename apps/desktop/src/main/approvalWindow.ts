export const APPROVAL_OPEN_CHANNEL = "general-agent:approval:open";
export const APPROVAL_COMPLETE_CHANNEL = "general-agent:approval:complete";

type IpcEvent = { sender: ApprovalWebContents };
type IpcHandler = (event: IpcEvent, value: unknown) => unknown;

export type ApprovalWebContents = {
  id: number;
  on(event: "will-navigate", listener: (event: { preventDefault(): void }, url: string) => void): void;
  setWindowOpenHandler(handler: () => { action: "deny" }): void;
};

export type ApprovalWindow = {
  webContents: ApprovalWebContents;
  close(): void;
  isDestroyed(): boolean;
  loadURL(url: string): Promise<void>;
  on(event: "closed", listener: () => void): void;
};

export type ApprovalWindowOptions = {
  height: number;
  modal: true;
  resizable: boolean;
  show: boolean;
  title: string;
  width: number;
  webPreferences: {
    contextIsolation: true;
    nodeIntegration: false;
    preload: string;
    sandbox: true;
  };
};

type ApprovalWindowControllerOptions = {
  approvalPreload: string;
  approvalUrl: string;
  createWindow(options: ApprovalWindowOptions): ApprovalWindow;
  ipcMain: {
    handle(channel: string, handler: IpcHandler): void;
    removeHandler(channel: string): void;
  };
  requesterWebContents: ApprovalWebContents;
};

type Completion = {
  approvalId: string;
  decision: "approve" | "reject";
};

type PendingWindow = {
  approvalId: string;
  resolve(value: unknown): void;
  window: ApprovalWindow;
};

export function registerApprovalWindowController(
  options: ApprovalWindowControllerOptions
): () => void {
  const byApproval = new Map<string, PendingWindow>();
  const byWebContents = new Map<number, PendingWindow>();

  options.ipcMain.handle(APPROVAL_OPEN_CHANNEL, async (event, value) => {
    if (event.sender !== options.requesterWebContents) {
      throw new Error("Approval requests must originate from the requester window");
    }
    const approvalId = approvalUuid(value);
    if (byApproval.has(approvalId)) throw new Error("Approval window is already open");
    const target = approvalTarget(options.approvalUrl, approvalId);
    const window = options.createWindow({
      height: 760,
      modal: true,
      resizable: true,
      show: true,
      title: "GeneralAgent Approval",
      width: 720,
      webPreferences: {
        contextIsolation: true,
        nodeIntegration: false,
        preload: options.approvalPreload,
        sandbox: true
      }
    });
    window.webContents.setWindowOpenHandler(() => ({ action: "deny" }));
    window.webContents.on("will-navigate", (navigation, url) => {
      if (url !== target) navigation.preventDefault();
    });
    const observed = new Promise<unknown>((resolve) => {
      const pending = { approvalId, resolve, window };
      byApproval.set(approvalId, pending);
      byWebContents.set(window.webContents.id, pending);
      window.on("closed", () => settle(pending, { approvalId, status: "closed" }));
    });
    try {
      await window.loadURL(target);
    } catch (error) {
      const pending = byApproval.get(approvalId);
      if (pending) settle(pending, { approvalId, status: "load_failed" });
      throw error;
    }
    return observed;
  });

  options.ipcMain.handle(APPROVAL_COMPLETE_CHANNEL, (event, value) => {
    const pending = byWebContents.get(event.sender.id);
    if (!pending || event.sender !== pending.window.webContents) {
      throw new Error("Approval completion must originate from the approval window");
    }
    const completion = parseCompletion(value);
    if (completion.approvalId !== pending.approvalId) {
      throw new Error("Approval completion identifier does not match the isolated window");
    }
    settle(pending, { ...completion, status: "completed" });
    if (!pending.window.isDestroyed()) pending.window.close();
    return { accepted: true };
  });

  function settle(pending: PendingWindow, value: unknown): void {
    if (byApproval.get(pending.approvalId) !== pending) return;
    byApproval.delete(pending.approvalId);
    byWebContents.delete(pending.window.webContents.id);
    pending.resolve(value);
  }

  return () => {
    options.ipcMain.removeHandler(APPROVAL_OPEN_CHANNEL);
    options.ipcMain.removeHandler(APPROVAL_COMPLETE_CHANNEL);
    for (const pending of byApproval.values()) {
      settle(pending, { approvalId: pending.approvalId, status: "disposed" });
      if (!pending.window.isDestroyed()) pending.window.close();
    }
  };
}

function approvalTarget(base: string, approvalId: string): string {
  const url = new URL(base);
  url.searchParams.set("approvalId", approvalId);
  return url.href;
}

function approvalUuid(value: unknown): string {
  if (typeof value !== "string" || !UUID_V4.test(value)) {
    throw new Error("Approval identifier is not allowed");
  }
  return value;
}

function parseCompletion(value: unknown): Completion {
  if (!isRecord(value)) throw new Error("Approval completion is invalid");
  const approvalId = approvalUuid(value.approvalId);
  const decision = value.decision;
  if (decision !== "approve" && decision !== "reject") {
    throw new Error("Approval decision is invalid");
  }
  return { approvalId, decision };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

const UUID_V4 = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
