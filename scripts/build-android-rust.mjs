import { copyFileSync, existsSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
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

export function runAndroidRustBuild() {
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

  const result = spawnSync("cargo", cargoArgs, {
    cwd: projectRoot,
    env: {
      ...process.env,
      AR_aarch64_linux_android: config.archiver,
      CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER: config.linker,
      CC_aarch64_linux_android: config.linker,
    },
    stdio: "inherit",
  });
  if (result.status !== 0) {
    throw new Error(`Android Rust build failed with exit code ${result.status ?? 1}`);
  }

  const { source, destination } = androidRustLibraryPaths(projectRoot, profile);
  mkdirSync(dirname(destination), { recursive: true });
  copyFileSync(source, destination);
  console.log(`Copied ${source} -> ${destination}`);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  try {
    runAndroidRustBuild();
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  }
}
