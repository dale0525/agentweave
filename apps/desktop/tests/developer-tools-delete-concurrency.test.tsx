import { act, cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  DevSkillInventory,
  DevSkillPackage,
  DevSkillReloadResponse
} from "../src/renderer/api";
import { DeveloperTools } from "../src/renderer/screens/DeveloperTools";

vi.mock("../src/renderer/components/developer/DeleteSkillDialog", () => ({
  DeleteSkillDialog: ({
    onConfirm,
    skillPackage
  }: {
    onConfirm: (skillPackage: DevSkillPackage) => Promise<void>;
    skillPackage: DevSkillPackage | null;
  }) =>
    skillPackage ? (
      <button onClick={() => void onConfirm(skillPackage)} type="button">
        Confirm delete {skillPackage.id}
      </button>
    ) : null
}));

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("DeveloperTools delete concurrency", () => {
  it("ignores stale delete success after a newer delete starts", async () => {
    const user = userEvent.setup();
    const staleDelete = deferred<Response>();
    const latestDelete = deferred<Response>();
    mockFetch([
      jsonResponse(inventoryWith("echo", "planning")),
      jsonResponse(reloadResponse(5, inventoryWith("echo", "planning"))),
      staleDelete.promise,
      latestDelete.promise
    ]);
    render(<DeveloperTools onBack={() => undefined} />);

    await user.click(await screen.findByRole("button", { name: "Reload diagnostics" }));
    await user.click(screen.getByRole("button", { name: "Delete package" }));
    await user.click(screen.getByRole("button", { name: "Confirm delete echo" }));
    await user.click(screen.getByRole("button", { name: /planning/i }));
    await user.click(screen.getByRole("button", { name: "Delete package" }));
    await user.click(screen.getByRole("button", { name: "Confirm delete planning" }));

    await settleDeferred(staleDelete, jsonResponse(inventoryWith("planning")));

    expectWorkbenchBusy(true);
    expect(screen.getByText("skills/planning")).toBeInTheDocument();
    expect(screen.getByText("Active snapshot 5")).toBeInTheDocument();

    await settleDeferred(latestDelete, jsonResponse(inventoryWith("echo")));
    expect(await screen.findByText("skills/echo")).toBeInTheDocument();
    expect(screen.getByText("Active snapshot 5")).toBeInTheDocument();
    expectWorkbenchBusy(false);
  });

  it("ignores stale delete failure after a newer delete starts", async () => {
    const user = userEvent.setup();
    const staleDelete = deferred<Response>();
    const latestDelete = deferred<Response>();
    mockFetch([
      jsonResponse(inventoryWith("echo", "planning")),
      jsonResponse(reloadResponse(5, inventoryWith("echo", "planning"))),
      staleDelete.promise,
      latestDelete.promise
    ]);
    render(<DeveloperTools onBack={() => undefined} />);

    await user.click(await screen.findByRole("button", { name: "Reload diagnostics" }));
    await user.click(screen.getByRole("button", { name: "Delete package" }));
    await user.click(screen.getByRole("button", { name: "Confirm delete echo" }));
    await user.click(screen.getByRole("button", { name: /planning/i }));
    await user.click(screen.getByRole("button", { name: "Delete package" }));
    await user.click(screen.getByRole("button", { name: "Confirm delete planning" }));

    await rejectDeferred(staleDelete, new Error("stale delete failed"));

    expectWorkbenchBusy(true);
    expect(screen.getByText("skills/planning")).toBeInTheDocument();
    expect(
      screen.queryByText("Action failed. Keep the current inventory and try again.")
    ).not.toBeInTheDocument();

    await settleDeferred(latestDelete, jsonResponse(inventoryWith("echo")));
    expect(await screen.findByText("skills/echo")).toBeInTheDocument();
    expect(screen.getByText("Active snapshot 5")).toBeInTheDocument();
    expectWorkbenchBusy(false);
  });
});

function skillPackage(id: string): DevSkillPackage {
  return {
    id,
    path: id,
    name: id,
    description: `${id} package.`,
    hasSkillMd: true,
    hasRuntimeManifest: true,
    runtimeTools: [`${id}_tool`],
    packageKind: "combined",
    bundleReady: true,
    runtimeReady: true,
    instructionReady: true,
    releaseReady: true,
    readinessIssues: [],
    requiredRuntimeTools: [],
    requiredConnectors: [],
    hasPackageMetadata: true,
    validation: { ok: true, errors: [], warnings: [] }
  };
}

function inventoryWith(...ids: string[]): DevSkillInventory {
  return {
    root: "/repo/skills",
    packages: ids.map(skillPackage)
  };
}

function reloadResponse(
  activeGeneration: number,
  inventory: DevSkillInventory
): DevSkillReloadResponse {
  return {
    inventory,
    previousGeneration: activeGeneration - 1,
    activeGeneration,
    activePackages: inventory.packages.length,
    inactivePackages: 0,
    reloadStatus: "published"
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, reject, resolve };
}

async function settleDeferred<T>(pending: ReturnType<typeof deferred<T>>, value: T) {
  await act(async () => {
    pending.resolve(value);
    await Promise.resolve();
    await Promise.resolve();
  });
}

async function rejectDeferred<T>(pending: ReturnType<typeof deferred<T>>, error: Error) {
  await act(async () => {
    pending.reject(error);
    await Promise.resolve();
    await Promise.resolve();
  });
}

function expectWorkbenchBusy(isBusy: boolean) {
  const workbench = screen
    .getByRole("main", { name: "Developer Tools" })
    .querySelector("[aria-busy]");
  expect(workbench).toHaveAttribute("aria-busy", String(isBusy));
}

function jsonResponse(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    headers: { "Content-Type": "application/json" },
    status: 200
  });
}

function mockFetch(responses: Array<Response | Promise<Response>>) {
  const fetchMock = vi.fn();
  for (const response of responses) {
    fetchMock.mockResolvedValueOnce(response);
  }
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}
