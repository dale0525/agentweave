export type OwnerMode =
  | "disabled"
  | "diagnostics_only"
  | "owner_only"
  | "organization_managed";

export type OwnerPolicy = {
  mode: OwnerMode;
  actorId: string;
  grants: string[];
  activation_approval_required?: boolean;
  permission_escalation_approval_required?: boolean;
  rollback_approval_required?: boolean;
};

export const disabledOwnerPolicy: OwnerPolicy = {
  mode: "disabled",
  actorId: "anonymous",
  grants: []
};

export async function getOwnerPolicy(): Promise<OwnerPolicy> {
  const bridge = getOwnerBridge();
  if (!bridge) {
    return disabledOwnerPolicy;
  }
  return bridge.ownerPolicy() as Promise<OwnerPolicy>;
}

export async function ownerRequest<T>(
  path: string,
  init: RequestInit
): Promise<T> {
  const bridge = getOwnerBridge();
  if (!bridge) {
    throw new Error("Owner skill management is disabled");
  }
  return bridge.ownerRequest(path, init) as Promise<T>;
}

export function canInspectOwnerSkills(policy: OwnerPolicy | null): boolean {
  if (!policy || !policy.grants.includes("inspect")) {
    return false;
  }
  return policy.mode !== "disabled";
}

export function canManageOwnerSkills(policy: OwnerPolicy, grant: string): boolean {
  return policy.mode === "owner_only" && policy.grants.includes(grant);
}

function getOwnerBridge(): NonNullable<Window["generalAgent"]> | null {
  return window.generalAgent ?? createVisualOwnerBridge();
}

function createVisualOwnerBridge(): NonNullable<Window["generalAgent"]> | null {
  if (!import.meta.env.DEV) return null;
  const scenario = new URLSearchParams(window.location.search).get("ownerMock");
  if (!scenario) return null;

  const validationOk = { ok: true, errors: [], warnings: [] };
  const validationError = {
    ok: false,
    errors: ["Instruction heading is required", "Unknown host tool: calendar.write"],
    warnings: []
  };
  const activeRevision = {
    revision_id: "22222222-2222-4222-8222-222222222222",
    version: "2.0.0",
    status: "active",
    created_by: "owner-1",
    created_at: "2026-07-12T10:00:00Z",
    kind: "instruction_only",
    instructions: "- Review daily calendar\n+ Review calendar and summarize conflicts",
    validation: validationOk,
    required_tools: ["calendar.read"],
    required_capabilities: [],
    required_connectors: ["local.calendar"],
    dependencies: [],
    permission_diff: { capabilities: { added: [] } }
  };
  const previousRevision = {
    ...activeRevision,
    revision_id: "11111111-1111-4111-8111-111111111111",
    version: "1.0.0",
    status: "managed",
    instructions: "# Calendar"
  };
  const draftRevision = {
    ...activeRevision,
    revision_id: "33333333-3333-4333-8333-333333333333",
    version: "2.1.0",
    status: "draft",
    instructions: "Keep this draft content",
    validation: validationError
  };
  const managed = {
    package_id: "com.example.calendar",
    display_name: "Calendar Operations",
    version: scenario === "draft-error" ? "2.1.0" : "2.0.0",
    source_layer: "managed",
    status: scenario === "draft-error" ? "draft" : "active",
    reason: "",
    active_revision_id:
      scenario === "draft-error" ? draftRevision.revision_id : activeRevision.revision_id,
    kind: "instruction_only",
    validation: scenario === "draft-error" ? validationError : validationOk,
    requirements: {
      runtime_tools: ["calendar.read"],
      capabilities: [],
      connectors: ["local.calendar"],
      packages: []
    },
    revisions:
      scenario === "draft-error"
        ? [draftRevision, activeRevision, previousRevision]
        : [activeRevision, previousRevision]
  };
  const builtIn = {
    package_id: "org.generalagent.core.files",
    display_name: "File Operations",
    version: "1.4.0",
    source_layer: "builtin",
    status: "active",
    reason: "",
    active_revision_id: null,
    kind: "host_tools_only",
    validation: validationOk,
    requirements: {
      runtime_tools: ["read_file", "write_file"],
      capabilities: ["filesystem.read"],
      connectors: [],
      packages: []
    }
  };

  return {
    ownerPolicy: async () =>
      scenario === "disabled"
        ? disabledOwnerPolicy
        : {
            mode: "owner_only",
            actorId: "owner-1",
            grants: [
              "inspect",
              "create_draft",
              "edit_draft",
              "validate",
              "test",
              "activate",
              "rollback",
              "disable",
              "delete_managed"
            ]
          },
    ownerRequest: async (path, init) => {
      if (path === "/owner/skills") {
        return { effective: [managed, builtIn], managed: [] };
      }
      if (path.endsWith("/activation")) {
        return {
          approval_id: "approval-1",
          package_id: managed.package_id,
          permission_diff: { capabilities: { added: [] } },
          requested_by: "owner-1",
          revision_id: managed.active_revision_id,
          status: "pending"
        };
      }
      if (path.includes("/approvals/")) {
        return { status: "approved", active_generation: 4 };
      }
      if (path.endsWith("/validate")) {
        return {
          ...validationError,
          requiredTools: ["calendar.read"],
          requiredConnectors: ["local.calendar"],
          dependencies: [],
          requiredCapabilities: [],
          permissionDiff: {},
          revisionId: draftRevision.revision_id,
          snapshotGeneration: 3
        };
      }
      if (init.method === "PUT") {
        return {
          package_id: managed.package_id,
          revision_id: draftRevision.revision_id,
          version: draftRevision.version,
          kind: draftRevision.kind,
          validation: { status: "pending" },
          status: "draft"
        };
      }
      return {};
    }
  };
}
