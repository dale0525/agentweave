export type RequesterWindowWebContents = {
  on(
    event: "will-navigate",
    listener: (event: { preventDefault(): void }, url: string) => void
  ): unknown;
  setWindowOpenHandler(
    handler: (details: { url: string }) => { action: "deny" }
  ): unknown;
};

type RequesterWindowSecurityOptions = {
  openExternal(url: string): Promise<unknown> | unknown;
  onExternalError?: (error: unknown) => void;
  trustedUrl: string;
  webContents: RequesterWindowWebContents;
};

export function configureRequesterWindowSecurity(
  options: RequesterWindowSecurityOptions
): void {
  const trusted = new URL(options.trustedUrl);
  if (trusted.protocol !== "file:" && trusted.protocol !== "http:" && trusted.protocol !== "https:") {
    throw new Error("Requester window trusted URL scheme is not allowed");
  }
  options.webContents.on("will-navigate", (event, candidate) => {
    if (!isTrustedNavigation(trusted, candidate)) event.preventDefault();
  });
  options.webContents.setWindowOpenHandler(({ url }) => {
    if (isExternalUrl(url)) {
      void Promise.resolve(options.openExternal(url)).catch((error) => {
        options.onExternalError?.(error);
      });
    }
    return { action: "deny" };
  });
}

function isTrustedNavigation(trusted: URL, candidate: string): boolean {
  let target: URL;
  try {
    target = new URL(candidate);
  } catch {
    return false;
  }
  if (trusted.protocol === "file:") return target.href === trusted.href;
  return target.protocol === trusted.protocol && target.origin === trusted.origin;
}

function isExternalUrl(candidate: string): boolean {
  try {
    const protocol = new URL(candidate).protocol;
    return protocol === "http:" || protocol === "https:";
  } catch {
    return false;
  }
}
