import { spawn } from "node:child_process";
import { randomBytes, randomUUID } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HANDSHAKE_LIMIT = 4_096;
const LOG_LIMIT = 32_768;
const START_TIMEOUT_MS = 20_000;
const STOP_TIMEOUT_MS = 5_000;

function fail(message) {
  throw new Error(message);
}

function requireDirectory(path, label) {
  if (!existsSync(path) || !statSync(path).isDirectory()) fail(`${label} is missing`);
}

function requireFile(path, label) {
  if (!existsSync(path) || !statSync(path).isFile()) fail(`${label} is missing`);
}

function readJson(path, label) {
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch (error) {
    fail(`${label} is invalid JSON: ${error instanceof Error ? error.message : String(error)}`);
  }
}

function requireString(value, label) {
  if (typeof value !== "string" || value.length === 0) fail(`${label} is invalid`);
  return value;
}

function requireStringArray(value, label) {
  if (!Array.isArray(value) || value.some((entry) => typeof entry !== "string")) {
    fail(`${label} is invalid`);
  }
  return value;
}

export function packagedSidecarPlan(appPath) {
  const bundleRoot = resolve(appPath);
  requireDirectory(bundleRoot, "packaged macOS App");
  if (!basename(bundleRoot).endsWith(".app")) fail("packaged macOS App must use the .app suffix");
  const resourcesRoot = join(bundleRoot, "Contents", "Resources");
  const appRoot = join(resourcesRoot, "agent-app", "app");
  const skillsRoot = join(resourcesRoot, "skills");
  const sidecarPath = join(resourcesRoot, "sidecar", "agent-server");
  requireDirectory(appRoot, "packaged Agent App");
  requireDirectory(skillsRoot, "packaged first-party skills");
  requireFile(sidecarPath, "packaged sidecar");
  if ((statSync(sidecarPath).mode & 0o111) === 0) fail("packaged sidecar is not executable");

  const manifest = readJson(join(appRoot, "agent-app.json"), "packaged Agent App manifest");
  const requirements = manifest.requires;
  if (!requirements || typeof requirements !== "object") fail("packaged requirements are invalid");
  return Object.freeze({
    appRoot,
    bundleRoot,
    expected: Object.freeze({
      appId: requireString(manifest.appId, "packaged App identifier"),
      capabilities: Object.freeze([...requireStringArray(requirements.capabilities, "packaged capabilities")]),
      connectors: Object.freeze([...requireStringArray(requirements.connectors, "packaged connectors")]),
      displayName: requireString(manifest.branding?.displayName, "packaged display name"),
      packageId: requireString(manifest.package?.id, "packaged package identifier"),
      runtimeTools: Object.freeze([...requireStringArray(requirements.runtimeTools, "packaged runtime tools")]),
      version: requireString(manifest.package?.version, "packaged App version"),
    }),
    resourcesRoot,
    sidecarPath,
    skillsRoot,
  });
}

export function assertPackagedDiscovery(discovery, expected) {
  if (!discovery || typeof discovery !== "object") fail("host bootstrap response is invalid");
  if (discovery.schemaVersion !== 1 || discovery.platform !== "desktop") {
    fail("host bootstrap platform contract is invalid");
  }
  const identity = discovery.identity;
  if (
    !identity
    || identity.appId !== expected.appId
    || identity.packageId !== expected.packageId
    || identity.version !== expected.version
    || identity.displayName !== expected.displayName
  ) {
    fail("host bootstrap identity does not match the packaged manifest");
  }
  const requirements = discovery.requirements;
  if (!requirements || typeof requirements !== "object") {
    fail("host bootstrap requirements are invalid");
  }
  assertStringSet(requirements.capabilities, expected.capabilities, "capabilities");
  assertStringSet(requirements.runtimeTools, expected.runtimeTools, "runtime tools");
  assertStringSet(requirements.connectors, expected.connectors, "connectors");
  return true;
}

function assertStringSet(actual, expected, label) {
  if (!Array.isArray(actual) || actual.some((entry) => typeof entry !== "string")) {
    fail(`host bootstrap ${label} are invalid`);
  }
  const left = [...actual].sort();
  const right = [...expected].sort();
  if (left.length !== right.length || left.some((entry, index) => entry !== right[index])) {
    fail(`host bootstrap ${label} do not match the packaged manifest`);
  }
}

export async function checkPackagedSidecar(appPath) {
  const plan = packagedSidecarPlan(appPath);
  const temporaryRoot = mkdtempSync(join(tmpdir(), "agentweave-packaged-sidecar-"));
  let child;
  try {
    for (const directory of ["cache", "data", "workspace"]) {
      mkdirSync(join(temporaryRoot, directory), { recursive: true });
    }
    const launch = launchSidecar(plan, temporaryRoot);
    child = launch.child;
    const handshake = await launch.handshake;
    await expectResponse(handshake.origin, null, "/health", 401);
    await expectResponse(handshake.origin, handshake.token, "/health", 200, "ok");
    const discovery = await expectJson(handshake.origin, handshake.token, "/host/bootstrap");
    assertPackagedDiscovery(discovery, plan.expected);
    await stopChild(child);
    child = undefined;
    return plan;
  } finally {
    if (child) await stopChild(child);
    rmSync(temporaryRoot, { force: true, recursive: true });
  }
}

function launchSidecar(plan, temporaryRoot) {
  const launchId = randomUUID();
  const token = randomBytes(32).toString("base64url");
  let stderr = "";
  const child = spawn(plan.sidecarPath, [], {
    cwd: dirname(plan.bundleRoot),
    env: childEnvironment(plan, temporaryRoot),
    stdio: ["ignore", "ignore", "pipe", "pipe", "pipe"],
    windowsHide: true,
  });
  child.stdio[2].on("data", (chunk) => {
    if (stderr.length < LOG_LIMIT) stderr += chunk.toString().slice(0, LOG_LIMIT - stderr.length);
  });
  child.stdio[3].end(JSON.stringify({
    schemaVersion: 1,
    launchId,
    transportToken: token,
  }));
  return {
    child,
    handshake: readHandshake(child, launchId, token, () => stderr.trim()),
  };
}

function childEnvironment(plan, temporaryRoot) {
  const env = {};
  for (const key of [
    "DYLD_FALLBACK_LIBRARY_PATH",
    "DYLD_LIBRARY_PATH",
    "HOME",
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "LANG",
    "NO_PROXY",
    "PATH",
    "RUST_BACKTRACE",
    "RUST_LOG",
    "SSL_CERT_DIR",
    "SSL_CERT_FILE",
    "TMPDIR",
    "TZ",
  ]) {
    if (process.env[key]) env[key] = process.env[key];
  }
  return {
    ...env,
    AGENTWEAVE_APP_DATA_ROOT: join(temporaryRoot, "data"),
    AGENTWEAVE_APP_ROOT: plan.appRoot,
    AGENTWEAVE_BUILTIN_SKILLS_MODE: "directory",
    AGENTWEAVE_CACHE_ROOT: join(temporaryRoot, "cache"),
    AGENTWEAVE_DATABASE_URL: `sqlite://${join(temporaryRoot, "data", "agentweave.db")}?mode=rwc`,
    AGENTWEAVE_FAKE_MAIL: "enabled",
    AGENTWEAVE_LAUNCH_CONFIG_FD: "3",
    AGENTWEAVE_LAUNCH_RESULT_FD: "4",
    AGENTWEAVE_MANAGED_SKILLS: "1",
    AGENTWEAVE_SKILLS_ROOT: plan.skillsRoot,
    AGENTWEAVE_WORKSPACE_ROOT: join(temporaryRoot, "workspace"),
  };
}

function readHandshake(child, launchId, token, readStderr) {
  return new Promise((resolveHandshake, rejectHandshake) => {
    let settled = false;
    let data = "";
    const timer = setTimeout(() => finish(new Error("packaged sidecar launch handshake timed out")), START_TIMEOUT_MS);
    const finish = (error, value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      if (error) rejectHandshake(withLogs(error, readStderr()));
      else resolveHandshake(value);
    };
    child.once("error", () => finish(new Error("packaged sidecar failed to spawn")));
    child.once("exit", (code, signal) => {
      finish(new Error(`packaged sidecar exited before launch handshake (${signal ?? code ?? "unknown"})`));
    });
    child.stdio[4].on("data", (chunk) => {
      data += chunk.toString();
      if (data.length > HANDSHAKE_LIMIT) return finish(new Error("packaged sidecar launch handshake is too large"));
      const newline = data.indexOf("\n");
      if (newline < 0) return;
      let result;
      try {
        result = JSON.parse(data.slice(0, newline));
      } catch {
        return finish(new Error("packaged sidecar launch handshake is invalid"));
      }
      if (
        result.schemaVersion !== 1
        || result.launchId !== launchId
        || result.pid !== child.pid
        || typeof result.origin !== "string"
        || Object.hasOwn(result, "transportToken")
      ) {
        return finish(new Error("packaged sidecar returned an invalid launch handshake"));
      }
      let origin;
      try {
        origin = new URL(result.origin);
      } catch {
        return finish(new Error("packaged sidecar returned an invalid dynamic origin"));
      }
      if (origin.protocol !== "http:" || origin.hostname !== "127.0.0.1" || origin.pathname !== "/") {
        return finish(new Error("packaged sidecar returned an unsafe dynamic origin"));
      }
      return finish(null, { origin: origin.href, token });
    });
  });
}

function withLogs(error, logs) {
  return logs ? new Error(`${error.message}\n${logs}`) : error;
}

async function expectResponse(origin, token, path, expectedStatus, expectedBody) {
  const headers = token ? { "X-AgentWeave-Transport": token } : {};
  const response = await fetch(new URL(path, origin), {
    headers,
    signal: AbortSignal.timeout(5_000),
  });
  if (response.status !== expectedStatus) {
    fail(`expected ${path} HTTP ${expectedStatus}, received ${response.status}`);
  }
  if (expectedBody !== undefined && await response.text() !== expectedBody) {
    fail(`${path} response body is invalid`);
  }
}

async function expectJson(origin, token, path) {
  const response = await fetch(new URL(path, origin), {
    headers: { "X-AgentWeave-Transport": token },
    signal: AbortSignal.timeout(5_000),
  });
  if (response.status !== 200) fail(`expected ${path} HTTP 200, received ${response.status}`);
  try {
    return await response.json();
  } catch {
    fail(`${path} response body is invalid JSON`);
  }
}

async function stopChild(child) {
  if (child.exitCode !== null || child.signalCode !== null) return;
  const exited = new Promise((resolveExit) => child.once("exit", resolveExit));
  child.kill("SIGTERM");
  const stopped = await Promise.race([
    exited.then(() => true),
    new Promise((resolveWait) => setTimeout(() => resolveWait(false), STOP_TIMEOUT_MS)),
  ]);
  if (!stopped && child.exitCode === null && child.signalCode === null) {
    child.kill("SIGKILL");
    await exited;
    fail("packaged sidecar did not stop after SIGTERM");
  }
}

function parseArgs(argv) {
  const args = {};
  for (let index = 0; index < argv.length; index += 1) {
    const option = argv[index];
    if (option === "--help" || option === "-h") return { help: true };
    if (option === "--print-plan") {
      args.printPlan = true;
      continue;
    }
    if (option !== "--app") fail(`unknown argument '${option}'`);
    const value = argv[index + 1];
    if (!value || value.startsWith("--")) fail("--app requires a value");
    args.app = value;
    index += 1;
  }
  return args;
}

function usage() {
  return "Usage: node scripts/check-packaged-sidecar.mjs --app <path-to-app> [--print-plan]";
}

export async function runCli(argv = process.argv.slice(2)) {
  const args = parseArgs(argv);
  if (args.help) return console.log(usage());
  if (!args.app) fail(`--app is required\n${usage()}`);
  if (args.printPlan) {
    const plan = packagedSidecarPlan(args.app);
    console.log(JSON.stringify({
      bundleRoot: plan.bundleRoot,
      expected: plan.expected,
      sidecarPath: plan.sidecarPath,
    }, null, 2));
    return;
  }
  const plan = await checkPackagedSidecar(args.app);
  console.log(`Packaged sidecar check passed for ${plan.expected.displayName}`);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  runCli().catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  });
}
