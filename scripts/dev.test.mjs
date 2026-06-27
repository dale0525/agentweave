import assert from "node:assert/strict";
import test from "node:test";

import { createDevProcesses } from "./dev.mjs";

test("dev command starts the API server and desktop app", () => {
  const processes = createDevProcesses();

  assert.deepEqual(processes, [
    {
      name: "server",
      command: "cargo",
      args: ["run", "-p", "agent-server"]
    },
    {
      name: "desktop",
      command: "npm",
      args: [
        "--prefix",
        "apps/desktop",
        "run",
        "dev",
        "--",
        "--host",
        "127.0.0.1",
        "--port",
        "5173"
      ]
    }
  ]);
});
