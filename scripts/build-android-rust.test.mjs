import assert from "node:assert/strict";
import test from "node:test";

import {
  androidRustLibraryPaths,
  androidNdkHostTag,
  createAndroidRustBuildConfig,
} from "./build-android-rust.mjs";

test("Android NDK host tag follows the supported host platform", () => {
  assert.equal(androidNdkHostTag("darwin"), "darwin-x86_64");
  assert.equal(androidNdkHostTag("linux"), "linux-x86_64");
  assert.throws(
    () => androidNdkHostTag("win32"),
    /unsupported Android NDK host platform: win32/,
  );
});

test("Android Rust build validates required toolchain paths", () => {
  const existing = new Set([
    "/ndk",
    "/ndk/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android31-clang",
    "/ndk/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-ar",
    "/rustlib/aarch64-linux-android/lib",
  ]);
  const config = createAndroidRustBuildConfig({
    projectRoot: "/project",
    ndkRoot: "/ndk",
    platform: "linux",
    targetLibdir: "/rustlib/aarch64-linux-android/lib",
    pathExists: (path) => existing.has(path),
  });

  assert.equal(config.hostTag, "linux-x86_64");
  assert.equal(
    config.linker,
    "/ndk/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android31-clang",
  );
  assert.equal(
    config.archiver,
    "/ndk/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-ar",
  );
});

test("Android Rust build reports a missing target standard library", () => {
  assert.throws(
    () => createAndroidRustBuildConfig({
      projectRoot: "/project",
      ndkRoot: "/ndk",
      platform: "darwin",
      targetLibdir: "/missing/rustlib/aarch64-linux-android/lib",
      pathExists: (path) => path === "/ndk"
        || path.endsWith("aarch64-linux-android31-clang")
        || path.endsWith("llvm-ar"),
    }),
    /Android Rust target std not found.*rust-std-aarch64-linux-android/,
  );
});

test("Android Rust artifacts use variant-specific JNI directories", () => {
  assert.deepEqual(androidRustLibraryPaths("/project", "debug"), {
    source: "/project/target/aarch64-linux-android/debug/libmobile_ffi.so",
    destination: "/project/apps/android/app/build/generated/rustJniLibs/debug/arm64-v8a/libmobile_ffi.so",
  });
  assert.deepEqual(androidRustLibraryPaths("/project", "release"), {
    source: "/project/target/aarch64-linux-android/release/libmobile_ffi.so",
    destination: "/project/apps/android/app/build/generated/rustJniLibs/release/arm64-v8a/libmobile_ffi.so",
  });
});
