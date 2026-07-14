import { createHash } from "node:crypto";
import {
  copyFileSync,
  existsSync,
  lstatSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

import {
  PROJECT_ROOT,
  resolveConfinedPath,
  validateAgentApp,
} from "./scaffold-agent-app.mjs";

export const AGENT_APP_LOCK_SCHEMA_VERSION = 1;
const DEFAULT_RUNTIME_VERSION = "0.1.0";
const LOCK_FILE = "agent-app.lock.json";
const FORBIDDEN_RELEASE_FILE = /(^|\/)(\.env(?:\..*)?|.*\.(?:key|p12|pem|pfx)|credentials?\.json|secrets?\.json)$/i;

function fail(message) {
  throw new Error(message);
}

function normalized(path) {
  return path.split(sep).join("/");
}

function requireSemver(value, label) {
  if (typeof value !== "string" || !/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(value)) {
    fail(`${label} must be a semantic version`);
  }
}

function listRegularFiles(root, prefix = "") {
  const directory = prefix ? join(root, prefix) : root;
  const files = [];
  for (const entry of readdirSync(directory, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
    const path = prefix ? join(prefix, entry.name) : entry.name;
    const absolute = join(root, path);
    if (entry.isSymbolicLink() || lstatSync(absolute).isSymbolicLink()) {
      fail(`release input contains symbolic link '${normalized(path)}'`);
    }
    if (entry.isDirectory()) files.push(...listRegularFiles(root, path));
    else if (entry.isFile()) files.push(path);
    else fail(`release input contains unsupported entry '${normalized(path)}'`);
  }
  return files.sort((left, right) => normalized(left).localeCompare(normalized(right)));
}

function inspectTree(root, { excluded = new Set(), overrides = new Map() } = {}) {
  const files = listRegularFiles(root).filter((path) => !excluded.has(normalized(path)));
  if (files.length === 0) fail(`release input '${root}' is empty`);
  const digest = createHash("sha256");
  for (const path of files) {
    const portable = normalized(path);
    if (FORBIDDEN_RELEASE_FILE.test(portable)) {
      fail(`release input contains forbidden credential file '${portable}'`);
    }
    const bytes = overrides.get(portable) ?? readFileSync(join(root, path));
    digest.update(portable, "utf8");
    digest.update("\0", "utf8");
    digest.update(String(bytes.length), "ascii");
    digest.update("\0", "utf8");
    digest.update(bytes);
  }
  return { contentHash: digest.digest("hex"), files, overrides };
}

function copyInspectedTree(root, inspection, destination) {
  mkdirSync(destination, { recursive: true });
  for (const path of inspection.files) {
    const target = join(destination, path);
    mkdirSync(dirname(target), { recursive: true });
    const replacement = inspection.overrides?.get(normalized(path));
    if (replacement) writeFileSync(target, replacement);
    else copyFileSync(join(root, path), target);
  }
}

function readJson(path, label) {
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch (error) {
    fail(`${label} is invalid JSON: ${error instanceof Error ? error.message : String(error)}`);
  }
}

function localPackageSources(appRoot) {
  const packagesRoot = join(appRoot, "packages");
  const result = new Map();
  if (!existsSync(packagesRoot)) return result;
  for (const entry of readdirSync(packagesRoot, { withFileTypes: true })) {
    if (!entry.isDirectory() || entry.isSymbolicLink()) continue;
    const root = join(packagesRoot, entry.name);
    const manifestPath = join(root, "general-agent.json");
    if (!existsSync(manifestPath)) continue;
    const manifest = readJson(manifestPath, `${entry.name} package manifest`);
    if (result.has(manifest.id)) fail(`Agent App contains duplicate package '${manifest.id}'`);
    result.set(manifest.id, { root, source: "app" });
  }
  return result;
}

function selectedPackageSources(appRoot, app, catalog) {
  const appPackages = localPackageSources(appRoot);
  const catalogById = new Map(catalog.skills.map((skill) => [skill.id, skill]));
  return app.requires.packages.map((requirement) => {
    const local = appPackages.get(requirement.id);
    if (local) return { ...local, id: requirement.id };
    const catalogEntry = catalogById.get(requirement.id);
    if (!catalogEntry?.localPackage?.path) {
      fail(`selected package '${requirement.id}' has no distributable source`);
    }
    return {
      id: requirement.id,
      root: resolveConfinedPath(PROJECT_ROOT, catalogEntry.localPackage.path, `${requirement.id} source`),
      source: "first_party",
    };
  });
}

function prepareOutput(output) {
  const root = resolveConfinedPath(PROJECT_ROOT, output, "release output");
  if (existsSync(root)) {
    if (!statSync(root).isDirectory()) fail("release output exists and is not a directory");
    if (readdirSync(root).length > 0) fail("release output directory must be empty");
  }
  return root;
}

function requestedLocales(value) {
  if (value === undefined || value === null || value === "") return null;
  const items = (Array.isArray(value) ? value : String(value).split(","))
    .map((item) => String(item).trim())
    .filter(Boolean);
  if (items.length === 0) fail("locale selection must not be empty");
  if (new Set(items).size !== items.length) fail("locale selection contains duplicates");
  return items;
}

function localizedAppInspection(appRoot, app, locales, defaultLocale) {
  const selectedIds = requestedLocales(locales);
  if (!selectedIds && !defaultLocale) {
    return { app, inspection: inspectTree(appRoot) };
  }
  if (!app.localization) fail("Agent App does not declare localization resources");
  const byId = new Map(app.localization.locales.map((entry) => [entry.id, entry]));
  const ids = selectedIds ?? app.localization.locales.map((entry) => entry.id);
  for (const id of ids) {
    if (!byId.has(id)) fail(`selected locale '${id}' is not declared by the Agent App`);
  }
  const packagedDefault = defaultLocale
    ?? (ids.includes(app.localization.defaultLocale) ? app.localization.defaultLocale : ids[0]);
  if (!ids.includes(packagedDefault)) {
    fail(`default locale '${packagedDefault}' must be included in the locale selection`);
  }
  const packagedApp = structuredClone(app);
  packagedApp.localization = {
    defaultLocale: packagedDefault,
    locales: ids.map((id) => byId.get(id)),
  };
  const selected = new Set(ids);
  const excluded = new Set(
    app.localization.locales
      .filter((entry) => !selected.has(entry.id))
      .map((entry) => entry.resource),
  );
  const manifestBytes = Buffer.from(`${JSON.stringify(packagedApp, null, 2)}\n`, "utf8");
  const overrides = new Map([["agent-app.json", manifestBytes]]);
  return {
    app: packagedApp,
    inspection: inspectTree(appRoot, { excluded, overrides }),
  };
}

export function packageAgentApp({
  input,
  output,
  runtimeVersion = DEFAULT_RUNTIME_VERSION,
  locales,
  defaultLocale,
}) {
  requireSemver(runtimeVersion, "runtime version");
  const appRoot = resolveConfinedPath(PROJECT_ROOT, input, "Agent App input");
  const { app: sourceApp, catalog } = validateAgentApp(appRoot);
  const localized = localizedAppInspection(appRoot, sourceApp, locales, defaultLocale);
  const app = localized.app;
  const appInspection = localized.inspection;
  const packages = selectedPackageSources(appRoot, app, catalog)
    .map((source) => {
      const inspection = inspectTree(source.root);
      const manifest = readJson(join(source.root, "general-agent.json"), `${source.id} package manifest`);
      if (manifest.id !== source.id) fail(`${source.id} package source has mismatched identity`);
      return {
        ...source,
        inspection,
        version: manifest.version,
      };
    })
    .sort((left, right) => left.id.localeCompare(right.id));
  const outputRoot = prepareOutput(output);
  const lock = {
    schemaVersion: AGENT_APP_LOCK_SCHEMA_VERSION,
    app: {
      appId: app.appId,
      packageId: app.package.id,
      version: app.package.version,
      contentHash: appInspection.contentHash,
      path: "app",
    },
    runtime: {
      compatibility: app.compatibility.runtime,
      packagedWithVersion: runtimeVersion,
    },
    platforms: [...app.compatibility.platforms].sort(),
    packages: packages.map((entry) => ({
      id: entry.id,
      version: entry.version,
      contentHash: entry.inspection.contentHash,
      path: `packages/${entry.id}`,
      source: entry.source,
    })),
    hostRequirements: {
      capabilities: [...app.requires.capabilities].sort(),
      runtimeTools: [...app.requires.runtimeTools].sort(),
      connectors: [...app.requires.connectors].sort().map((id) => ({ id, runtimeVersion })),
      providers: [...app.requires.capabilities]
        .filter((id) => id.includes("provider"))
        .sort()
        .map((id) => ({ id, runtimeVersion })),
    },
    localization: app.localization ? {
      defaultLocale: app.localization.defaultLocale,
      locales: app.localization.locales.map((entry) => entry.id),
    } : null,
  };
  mkdirSync(outputRoot, { recursive: true });
  copyInspectedTree(appRoot, appInspection, join(outputRoot, "app"));
  for (const entry of packages) {
    copyInspectedTree(entry.root, entry.inspection, join(outputRoot, "packages", entry.id));
  }
  writeFileSync(join(outputRoot, LOCK_FILE), `${JSON.stringify(lock, null, 2)}\n`, "utf8");
  validateAgentAppRelease(outputRoot);
  return { lock, outputRoot };
}

function requireReleasePath(root, value, label) {
  if (typeof value !== "string" || value === "" || value.includes("\\")) {
    fail(`${label} is invalid`);
  }
  return resolveConfinedPath(root, value, label);
}

export function validateAgentAppRelease(input) {
  const releaseRoot = resolveConfinedPath(PROJECT_ROOT, input, "release artifact");
  if (!existsSync(releaseRoot) || !statSync(releaseRoot).isDirectory()) {
    fail("release artifact is not a directory");
  }
  const topLevel = readdirSync(releaseRoot).sort();
  if (JSON.stringify(topLevel) !== JSON.stringify([LOCK_FILE, "app", "packages"])) {
    fail("release artifact contains an unexpected top-level entry");
  }
  const lock = readJson(join(releaseRoot, LOCK_FILE), "Agent App lock");
  if (lock.schemaVersion !== AGENT_APP_LOCK_SCHEMA_VERSION) {
    fail(`unsupported Agent App lock schema version '${lock.schemaVersion}'`);
  }
  requireSemver(lock.runtime?.packagedWithVersion, "locked runtime version");
  const appRoot = requireReleasePath(releaseRoot, lock.app?.path, "locked App path");
  const { app } = validateAgentApp(appRoot);
  if (app.appId !== lock.app.appId || app.package.id !== lock.app.packageId || app.package.version !== lock.app.version) {
    fail("locked App identity does not match packaged manifest");
  }
  const lockedLocalization = app.localization ? {
    defaultLocale: app.localization.defaultLocale,
    locales: app.localization.locales.map((entry) => entry.id),
  } : null;
  if (JSON.stringify(lockedLocalization) !== JSON.stringify(lock.localization ?? null)) {
    fail("locked App localization does not match packaged manifest");
  }
  if (inspectTree(appRoot).contentHash !== lock.app.contentHash) fail("packaged App content hash mismatch");
  const expectedIds = app.requires.packages.map((entry) => entry.id).sort();
  const lockedIds = lock.packages.map((entry) => entry.id).sort();
  if (JSON.stringify(expectedIds) !== JSON.stringify(lockedIds)) fail("locked package inventory mismatch");
  for (const entry of lock.packages) {
    const packageRoot = requireReleasePath(releaseRoot, entry.path, `${entry.id} locked path`);
    const manifest = readJson(join(packageRoot, "general-agent.json"), `${entry.id} package manifest`);
    if (manifest.id !== entry.id || manifest.version !== entry.version) {
      fail(`${entry.id} locked package identity mismatch`);
    }
    if (inspectTree(packageRoot).contentHash !== entry.contentHash) {
      fail(`${entry.id} packaged content hash mismatch`);
    }
  }
  return lock;
}

function usage() {
  return [
    "Usage:",
    "  node scripts/package-agent-app.mjs --input <app-path> --output <release-path> [--runtime-version <version>] [--locales <id,id>] [--default-locale <id>]",
    "  node scripts/package-agent-app.mjs --verify <release-path>",
  ].join("\n");
}

function parseArgs(argv) {
  const args = {};
  for (let index = 0; index < argv.length; index += 1) {
    const option = argv[index];
    if (option === "--help" || option === "-h") return { help: true };
    if (!["--input", "--output", "--runtime-version", "--locales", "--default-locale", "--verify"].includes(option)) {
      fail(`unknown argument '${option}'`);
    }
    const value = argv[index + 1];
    if (!value || value.startsWith("--")) fail(`${option} requires a value`);
    const field = option.slice(2)
      .replace("runtime-version", "runtimeVersion")
      .replace("default-locale", "defaultLocale");
    args[field] = value;
    index += 1;
  }
  return args;
}

export function runCli(argv = process.argv.slice(2)) {
  const args = parseArgs(argv);
  if (args.help) return console.log(usage());
  if (args.verify) {
    if (args.input || args.output || args.runtimeVersion || args.locales || args.defaultLocale) {
      fail("--verify cannot be combined with packaging options");
    }
    validateAgentAppRelease(args.verify);
    console.log(`Verified Agent App release at ${relative(PROJECT_ROOT, resolve(PROJECT_ROOT, args.verify))}`);
    return;
  }
  if (!args.input || !args.output) fail(`--input and --output are required\n${usage()}`);
  const result = packageAgentApp(args);
  console.log(`Packaged Agent App release at ${relative(PROJECT_ROOT, result.outputRoot)}`);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  try {
    runCli();
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
