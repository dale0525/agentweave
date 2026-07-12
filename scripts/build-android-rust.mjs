import {
  copyFileSync,
  chmodSync,
  existsSync,
  lstatSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { createHash } from "node:crypto";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const projectRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const target = "aarch64-linux-android";

export function androidNdkHostTag(platform) {
  if (platform === "darwin") return "darwin-x86_64";
  if (platform === "linux") return "linux-x86_64";
  throw new Error(`unsupported Android NDK host platform: ${platform}`);
}

export function createAndroidRustBuildConfig({
  projectRoot,
  ndkRoot,
  platform,
  targetLibdir,
  pathExists = existsSync,
}) {
  const hostTag = androidNdkHostTag(platform);
  const toolchain = join(ndkRoot, "toolchains", "llvm", "prebuilt", hostTag, "bin");
  const linker = join(toolchain, "aarch64-linux-android31-clang");
  const archiver = join(toolchain, "llvm-ar");
  const requiredPaths = [
    [ndkRoot, `Android NDK not found at ${ndkRoot}`],
    [linker, `Android NDK linker not found at ${linker}`],
    [archiver, `Android NDK archiver not found at ${archiver}`],
    [
      targetLibdir,
      `Android Rust target std not found at ${targetLibdir}; install rust-std-aarch64-linux-android`,
    ],
  ];
  for (const [path, message] of requiredPaths) {
    if (!pathExists(path)) throw new Error(message);
  }

  return { projectRoot, ndkRoot, hostTag, toolchain, linker, archiver, targetLibdir };
}

export function androidRustLibraryPaths(projectRoot, profile) {
  return {
    source: join(projectRoot, "target", target, profile, "libmobile_ffi.so"),
    destination: join(
      projectRoot,
      "apps",
      "android",
      "app",
      "build",
      "generated",
      "rustJniLibs",
      profile,
      "arm64-v8a",
      "libmobile_ffi.so",
    ),
  };
}

export function androidSkillAssetPaths(projectRoot) {
  const generatedRoot = join(
    projectRoot,
    "apps",
    "android",
    "app",
    "build",
    "generated",
    "skillAssets",
    "main",
  );
  const assetRoot = join(generatedRoot, "builtin-skills");
  return {
    generatedRoot,
    assetRoot,
    bundleRoot: join(assetRoot, "bundle"),
    hashFile: join(assetRoot, "bundle.sha256"),
  };
}

export function runAndroidBuildSequence({
  prepareSkills,
  buildRust,
  skillsOnly = false,
  rustOnly = false,
}) {
  if (skillsOnly && rustOnly) {
    throw new Error("--skills-only and --rust-only cannot be used together");
  }
  if (!rustOnly) prepareSkills();
  if (!skillsOnly) buildRust();
}

function resolveTargetLibdir() {
  const result = spawnSync(
    "rustc",
    ["--print", "target-libdir", "--target", target],
    { cwd: projectRoot, encoding: "utf8" },
  );
  if (result.status !== 0) {
    throw new Error(
      `failed to locate Android Rust target std: ${result.stderr?.trim() || "rustc failed"}`,
    );
  }
  return result.stdout.trim();
}

function prepareAndroidSkillAssets() {
  const result = prepareAndroidSkillAssetsAt(projectRoot, ({ sourceRoot, bundleRoot }) => {
    runChecked(
      "cargo",
      [
        "run",
        "-p",
        "agent-server",
        "--bin",
        "bundle-skills",
        "--",
        "--source",
        sourceRoot,
        "--output",
        bundleRoot,
        "--platform",
        "android",
      ],
      "Android skill bundle",
      { cwd: projectRoot, stdio: "inherit" },
    );
  });
  console.log(`Prepared verified Android skill assets at ${result.assetRoot}`);
}

export function prepareAndroidSkillAssetsAt(root, runBundle) {
  const paths = androidSkillAssetPaths(root);
  const sourceRoot = join(
    root,
    "apps",
    "android",
    "app",
    "build",
    "generated",
    "androidSkillSource",
  );
  makeTreeWritableNoFollow(paths.assetRoot);
  rmSync(paths.assetRoot, { recursive: true, force: true });
  rmSync(sourceRoot, { recursive: true, force: true });
  mkdirSync(paths.assetRoot, { recursive: true });
  mkdirSync(sourceRoot, { recursive: true });
  stageAndroidSkillSource(join(root, "skills"), sourceRoot);
  runBundle({ sourceRoot, bundleRoot: paths.bundleRoot });
  const contentHash = hashSkillBundle(paths.bundleRoot);
  writeFileSync(paths.hashFile, `${contentHash}\n`, { encoding: "utf8", mode: 0o600 });
  return { ...paths, sourceRoot, contentHash };
}

function makeTreeWritableNoFollow(root) {
  if (!existsSync(root)) return;
  const metadata = lstatSync(root);
  if (metadata.isSymbolicLink()) {
    return;
  }
  chmodSync(root, metadata.isDirectory() ? 0o700 : 0o600);
  if (metadata.isDirectory()) {
    for (const entry of readdirSync(root)) {
      makeTreeWritableNoFollow(join(root, entry));
    }
  }
}

function stageAndroidSkillSource(skillsRoot, outputRoot) {
  for (const entry of readdirSync(skillsRoot, { withFileTypes: true })) {
    const packageRoot = join(skillsRoot, entry.name);
    const metadata = lstatSync(packageRoot);
    if (metadata.isSymbolicLink()) {
      throw new Error(`Android skill source contains a symlink: ${packageRoot}`);
    }
    if (!metadata.isDirectory()) continue;
    const descriptorPath = join(packageRoot, "general-agent.json");
    if (!existsSync(descriptorPath)) continue;
    const descriptor = JSON.parse(readFileSync(descriptorPath, "utf8"));
    const platforms = descriptor?.compatibility?.platforms;
    if (Array.isArray(platforms) && platforms.length > 0 && !platforms.includes("android")) {
      continue;
    }
    copyRegularTree(packageRoot, join(outputRoot, entry.name));
  }
}

function copyRegularTree(source, destination) {
  const metadata = lstatSync(source);
  if (metadata.isSymbolicLink()) {
    throw new Error(`Android skill source contains a symlink: ${source}`);
  }
  if (metadata.isDirectory()) {
    mkdirSync(destination, { recursive: false });
    for (const entry of readdirSync(source)) {
      copyRegularTree(join(source, entry), join(destination, entry));
    }
  } else if (metadata.isFile()) {
    copyFileSync(source, destination);
  } else {
    throw new Error(`Android skill source contains a special file: ${source}`);
  }
}

function buildAndroidRustNative() {
  const ndkRoot = process.env.ANDROID_NDK_HOME
    ?? join(projectRoot, ".tool", "android-sdk", "ndk", "28.2.13676358");
  const config = createAndroidRustBuildConfig({
    projectRoot,
    ndkRoot,
    platform: process.platform,
    targetLibdir: resolveTargetLibdir(),
  });
  const profile = process.env.GENERAL_AGENT_ANDROID_RUST_PROFILE === "release"
    ? "release"
    : "debug";
  const cargoArgs = ["build", "-p", "mobile-ffi", "--target", target];
  if (profile === "release") cargoArgs.push("--release");

  runChecked("cargo", cargoArgs, "Android Rust build", {
    cwd: projectRoot,
    env: {
      ...process.env,
      AR_aarch64_linux_android: config.archiver,
      CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER: config.linker,
      CC_aarch64_linux_android: config.linker,
    },
    stdio: "inherit",
  });

  const { source, destination } = androidRustLibraryPaths(projectRoot, profile);
  mkdirSync(dirname(destination), { recursive: true });
  copyFileSync(source, destination);
  console.log(`Copied ${source} -> ${destination}`);
}

export function runAndroidRustBuild(options = {}) {
  runAndroidBuildSequence({
    prepareSkills: prepareAndroidSkillAssets,
    buildRust: buildAndroidRustNative,
    ...options,
  });
}

function runChecked(command, args, label, options) {
  const result = spawnSync(command, args, options);
  if (result.status !== 0) {
    throw new Error(`${label} failed with exit code ${result.status ?? 1}`);
  }
}

function hashSkillBundle(bundleRoot) {
  const files = collectRegularFiles(bundleRoot);
  const relativePaths = files.map((file) => relative(bundleRoot, file).split(sep).join("/"));
  if (!relativePaths.includes("current")) {
    throw new Error("Android skill bundle is missing current metadata");
  }
  if (!relativePaths.some((path) => path.endsWith("/skill-bundle.json"))) {
    throw new Error("Android skill bundle is missing its manifest");
  }
  if (!relativePaths.some((path) => path.endsWith("/skill-bundle.lock"))) {
    throw new Error("Android skill bundle is missing its lock file");
  }
  const digest = createHash("sha256");
  for (const [index, path] of relativePaths.entries()) {
    const bytes = readFileSync(files[index]);
    digest.update(path, "utf8");
    digest.update(Buffer.from([0]));
    digest.update(String(bytes.length), "ascii");
    digest.update(Buffer.from([0]));
    digest.update(bytes);
  }
  return digest.digest("hex");
}

function collectRegularFiles(root) {
  if (!existsSync(root) || !lstatSync(root).isDirectory()) {
    throw new Error(`Android skill bundle directory is missing: ${root}`);
  }
  const files = [];
  const visit = (directory) => {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name);
      const metadata = lstatSync(path);
      if (metadata.isSymbolicLink()) {
        throw new Error(`Android skill bundle contains a symlink: ${path}`);
      }
      if (metadata.isDirectory()) {
        visit(path);
      } else if (metadata.isFile()) {
        files.push(path);
      } else {
        throw new Error(`Android skill bundle contains a special file: ${path}`);
      }
    }
  };
  visit(root);
  return files.sort((left, right) => {
    const leftPath = relative(root, left).split(sep).join("/");
    const rightPath = relative(root, right).split(sep).join("/");
    if (leftPath < rightPath) return -1;
    if (leftPath > rightPath) return 1;
    return 0;
  });
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  try {
    const flags = new Set(process.argv.slice(2));
    for (const flag of flags) {
      if (flag !== "--skills-only" && flag !== "--rust-only") {
        throw new Error(`unknown argument: ${flag}`);
      }
    }
    runAndroidRustBuild({
      skillsOnly: flags.has("--skills-only"),
      rustOnly: flags.has("--rust-only"),
    });
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  }
}
