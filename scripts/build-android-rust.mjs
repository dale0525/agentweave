import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const projectRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const ndkRoot = process.env.ANDROID_NDK_HOME
  ?? join(projectRoot, ".tool", "android-sdk", "ndk", "28.2.13676358");
const toolchain = join(ndkRoot, "toolchains", "llvm", "prebuilt", "darwin-x86_64", "bin");
const linker = join(toolchain, "aarch64-linux-android31-clang");
const profile = process.env.GENERAL_AGENT_ANDROID_RUST_PROFILE === "release" ? "release" : "debug";
const cargoArgs = ["build", "-p", "mobile-ffi", "--target", "aarch64-linux-android"];
if (profile === "release") cargoArgs.push("--release");

const result = spawnSync("cargo", cargoArgs, {
  cwd: projectRoot,
  env: {
    ...process.env,
    AR_aarch64_linux_android: join(toolchain, "llvm-ar"),
    CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER: linker,
    CC_aarch64_linux_android: linker,
  },
  stdio: "inherit",
});
if (result.status !== 0) process.exit(result.status ?? 1);

const source = join(projectRoot, "target", "aarch64-linux-android", profile, "libmobile_ffi.so");
const destination = join(
  projectRoot,
  "apps",
  "android",
  "app",
  "build",
  "generated",
  "rustJniLibs",
  "arm64-v8a",
  "libmobile_ffi.so",
);
mkdirSync(dirname(destination), { recursive: true });
copyFileSync(source, destination);
console.log(`Copied ${source} -> ${destination}`);
