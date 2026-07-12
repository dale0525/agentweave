import react from "@vitejs/plugin-react";
import type { ProxyOptions } from "vite";
import { defineConfig } from "vitest/config";

import { normalizeOwnerRequest } from "./src/preload/ownerTransport";

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      "/__owner/requester": ownerProxy(process.env.GENERAL_AGENT_OWNER_TOKEN),
      "/__owner/approver": ownerProxy(process.env.GENERAL_AGENT_APPROVER_TOKEN)
    }
  },
  test: {
    environment: "jsdom",
    setupFiles: "./vitest.setup.ts"
  }
});

function ownerProxy(token: string | undefined): ProxyOptions {
  const headers: Record<string, string> = {};
  if (token) headers.Authorization = `Bearer ${token}`;
  return {
    target: "http://127.0.0.1:49321",
    changeOrigin: false,
    headers,
    rewrite: (path: string) => path.replace(/^\/__owner\/(requester|approver)/, ""),
    bypass: (request, response) => {
      const path = request.url?.replace(/^\/__owner\/(requester|approver)/, "") ?? "";
      try {
        normalizeOwnerRequest(path, request.method ?? "GET");
      } catch {
        response.statusCode = 403;
        response.end("Forbidden");
        return request.url;
      }
    },
    configure: (proxy) => {
      proxy.on("proxyReq", (proxyRequest) => {
        proxyRequest.removeHeader("authorization");
        proxyRequest.removeHeader("cookie");
        if (token) proxyRequest.setHeader("Authorization", `Bearer ${token}`);
      });
    }
  };
}
