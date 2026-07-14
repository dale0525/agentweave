import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

const LOCAL_DESKTOP_URL = "http://127.0.0.1:5173";

export function createDevProcesses() {
  return [
    {
      name: "server",
      command: "cargo",
      args: ["run", "-p", "agent-server", "--bin", "agent-server"],
      env: {
        AGENTWEAVE_DEV_API: "1"
      }
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
        "5173",
        "--strictPort"
      ]
    }
  ];
}

function runDev() {
  const children = [];
  let shuttingDown = false;

  console.log("Starting AgentWeave local dev environment...");
  console.log(`Desktop: ${LOCAL_DESKTOP_URL}`);
  console.log("API:     http://127.0.0.1:49321");
  console.log("Press Ctrl+C to stop both processes.\n");

  for (const processConfig of createDevProcesses()) {
    const child = spawn(processConfig.command, processConfig.args, {
      cwd: process.cwd(),
      env: {
        ...process.env,
        ...processConfig.env
      },
      stdio: ["ignore", "pipe", "pipe"]
    });
    children.push({ child, name: processConfig.name });

    child.stdout?.on("data", (chunk) => writePrefixed(processConfig.name, chunk));
    child.stderr?.on("data", (chunk) => writePrefixed(processConfig.name, chunk));

    child.on("exit", (code, signal) => {
      if (shuttingDown) {
        return;
      }

      shuttingDown = true;
      const reason = signal ? `signal ${signal}` : `exit code ${code}`;
      console.error(`\n${processConfig.name} stopped with ${reason}. Stopping dev environment.`);
      stopChildren(children);
      process.exitCode = code ?? 1;
    });
  }

  for (const signal of ["SIGINT", "SIGTERM"]) {
    process.on(signal, () => {
      if (shuttingDown) {
        return;
      }

      shuttingDown = true;
      console.log("\nStopping AgentWeave local dev environment...");
      stopChildren(children);
      process.exitCode = 0;
    });
  }
}

function writePrefixed(name, chunk) {
  const text = chunk.toString();
  for (const line of text.split(/\r?\n/)) {
    if (line.length > 0) {
      console.log(`[${name}] ${line}`);
    }
  }
}

function stopChildren(children) {
  for (const { child } of children) {
    if (!child.killed) {
      child.kill("SIGTERM");
    }
  }
}

if (process.argv.includes("--print-commands")) {
  console.log(JSON.stringify(createDevProcesses(), null, 2));
} else if (process.argv[1] === fileURLToPath(import.meta.url)) {
  runDev();
}
