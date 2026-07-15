import assert from "node:assert/strict";
import {
  chmodSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";
import test from "node:test";

import {
  assertPackagedDiscovery,
  packagedSidecarPlan,
} from "./check-packaged-sidecar.mjs";
import { PROJECT_ROOT } from "./scaffold-agent-app.mjs";

const TEST_ROOT = join(PROJECT_ROOT, ".tool");

function fixture() {
  mkdirSync(TEST_ROOT, { recursive: true });
  const root = mkdtempSync(join(TEST_ROOT, "packaged-sidecar-"));
  const bundleRoot = join(root, "Fixture.app");
  const resourcesRoot = join(bundleRoot, "Contents", "Resources");
  const appRoot = join(resourcesRoot, "agent-app", "app");
  const sidecarPath = join(resourcesRoot, "sidecar", "agent-server");
  mkdirSync(appRoot, { recursive: true });
  mkdirSync(join(resourcesRoot, "skills"), { recursive: true });
  mkdirSync(join(sidecarPath, ".."), { recursive: true });
  writeFileSync(sidecarPath, "fixture", "utf8");
  chmodSync(sidecarPath, 0o755);
  writeFileSync(join(appRoot, "agent-app.json"), `${JSON.stringify({
    appId: "com.example.fixture",
    package: { id: "com.example.fixture.app", version: "1.2.3" },
    requires: {
      capabilities: ["memory-provider", "host-tools"],
      connectors: ["fixture-connector"],
      runtimeTools: ["memory_search", "memory_get"],
    },
    branding: { displayName: "Fixture" },
  }, null, 2)}\n`, "utf8");
  return { bundleRoot, root };
}

test("packaged sidecar plan is bound to App resources and manifest identity", () => {
  const item = fixture();
  try {
    const plan = packagedSidecarPlan(item.bundleRoot);

    assert.equal(plan.bundleRoot, item.bundleRoot);
    assert.equal(plan.expected.appId, "com.example.fixture");
    assert.equal(plan.expected.packageId, "com.example.fixture.app");
    assert.equal(plan.expected.displayName, "Fixture");
    assert.deepEqual(plan.expected.capabilities, ["memory-provider", "host-tools"]);
  } finally {
    rmSync(item.root, { force: true, recursive: true });
  }
});

test("packaged discovery must match identity and declared capability sets", () => {
  const expected = {
    appId: "com.example.fixture",
    packageId: "com.example.fixture.app",
    version: "1.2.3",
    displayName: "Fixture",
    capabilities: ["host-tools", "memory-provider"],
    runtimeTools: ["memory_get", "memory_search"],
    connectors: ["fixture-connector"],
  };
  const discovery = {
    schemaVersion: 1,
    platform: "desktop",
    identity: {
      appId: "com.example.fixture",
      packageId: "com.example.fixture.app",
      version: "1.2.3",
      displayName: "Fixture",
    },
    requirements: {
      capabilities: ["memory-provider", "host-tools"],
      runtimeTools: ["memory_search", "memory_get"],
      connectors: ["fixture-connector"],
    },
  };

  assert.equal(assertPackagedDiscovery(discovery, expected), true);
  assert.throws(
    () => assertPackagedDiscovery({ ...discovery, identity: { ...discovery.identity, appId: "wrong" } }, expected),
    /identity does not match/,
  );
  assert.throws(
    () => assertPackagedDiscovery({
      ...discovery,
      requirements: { ...discovery.requirements, capabilities: ["memory-provider"] },
    }, expected),
    /capabilities do not match/,
  );
});

test("packaged sidecar plan rejects an incomplete App bundle", () => {
  mkdirSync(TEST_ROOT, { recursive: true });
  const root = mkdtempSync(join(TEST_ROOT, "packaged-sidecar-invalid-"));
  const bundleRoot = join(root, "Invalid.app");
  mkdirSync(bundleRoot, { recursive: true });
  try {
    assert.throws(() => packagedSidecarPlan(bundleRoot), /packaged Agent App is missing/);
  } finally {
    rmSync(root, { force: true, recursive: true });
  }
});
