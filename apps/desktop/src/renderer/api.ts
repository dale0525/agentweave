import { ModelSettings } from "./types";

const SERVER_ORIGIN = "http://127.0.0.1:49321";

export type ServerSession = {
  id: string;
  title: string;
};

export type RuntimeEvent = {
  arguments?: unknown;
  call_id?: string;
  message?: string;
  name?: string;
  result?: unknown;
  text?: string;
  turn_id?: string;
  type: string;
};

export type ServerMessage = {
  id: string;
  session_id: string;
  role: string;
  content: string;
  created_at: string;
};

export type PostMessageResponse = {
  accepted: boolean;
  user_message?: ServerMessage;
  assistant_message?: ServerMessage;
  events?: RuntimeEvent[];
};

export type ModelConnectionTestResponse = {
  ok: boolean;
  message: string;
};

export type OwnerSkillValidation = {
  ok: boolean;
  errors: string[];
  warnings: string[];
  requiredTools?: string[];
  requiredConnectors?: string[];
  dependencies?: string[];
  requiredCapabilities?: string[];
  permissionDiff?: unknown;
  revisionId?: string;
  snapshotGeneration?: number;
};

export type OwnerSkillRequirements = {
  runtime_tools: string[];
  capabilities: string[];
  connectors: string[];
  packages: string[];
};

export type OwnerSkillRevision = {
  revision_id: string;
  version: string;
  status: string;
  editable: boolean;
  created_by: string;
  created_at: string;
  kind: string;
  instructions: string;
  validation: OwnerSkillValidation;
  requirements: OwnerSkillRequirements;
  permission_diff?: unknown;
};

export type OwnerSkillPackageSummary = {
  package_id: string;
  display_name: string;
  version: string;
  source_layer: string;
  status: string;
  reason: string;
  active_revision_id: string | null;
  available: boolean;
  content_hash: string | null;
  manageable: boolean;
};

export type OwnerSkillActionFacts = {
  can_edit_draft: boolean;
  can_validate_draft: boolean;
  can_request_activation: boolean;
  can_disable: boolean;
  can_request_removal: boolean;
  can_rollback: boolean;
};

export type OwnerLayeredSkill = {
  package_id: string;
  effective: OwnerSkillPackageSummary | null;
  managed: OwnerSkillPackageSummary | null;
  built_in_collision: boolean;
  actions: OwnerSkillActionFacts;
};

export type OwnerSkillPackage = Omit<
  OwnerSkillPackageSummary,
  "available" | "content_hash" | "manageable"
> & {
  display_name: string;
  effective: OwnerSkillPackageSummary | null;
  managed: OwnerSkillPackageSummary | null;
  built_in_collision: boolean;
  actions: OwnerSkillActionFacts;
  revisions: OwnerSkillRevision[];
  editable_draft: OwnerSkillRevision | null;
};

export type OwnerSkillInventory = {
  effective: OwnerSkillPackageSummary[];
  managed: OwnerSkillPackageSummary[];
  packages: OwnerLayeredSkill[];
};

export type OwnerSkillDraftSummary = {
  package_id: string;
  revision_id: string;
  version: string;
  kind: string;
  validation: unknown;
  status: string;
};

export type OwnerSkillApproval = {
  approval_id: string;
  operation?: "activation" | "removal" | "rollback";
  package_id: string;
  permission_diff: unknown;
  requested_by: string;
  revision_id: string;
  status: string;
};

export type OwnerSkillMutationReport = {
  status?: string;
  active_generation?: number;
  active_revision_id?: string;
  generation?: number;
};

export type DevSkillPackageKind =
  | "runtime"
  | "instruction"
  | "combined"
  | "empty"
  | "invalid";

export type DevSkillValidation = {
  ok: boolean;
  errors: string[];
  warnings: string[];
};

export type DevSkillPackage = {
  id: string;
  path: string;
  name: string;
  description: string;
  hasSkillMd: boolean;
  hasRuntimeManifest: boolean;
  runtimeTools: string[];
  packageKind: DevSkillPackageKind;
  bundleReady: boolean;
  runtimeReady: boolean;
  instructionReady: boolean;
  releaseReady: boolean;
  readinessIssues: string[];
  requiredRuntimeTools: string[];
  requiredConnectors: string[];
  hasPackageMetadata: boolean;
  validation: DevSkillValidation;
};

export type DevSkillInventory = {
  root: string;
  packages: DevSkillPackage[];
};

export type DevSkillReloadResponse = {
  inventory: DevSkillInventory;
  previousGeneration: number;
  activeGeneration: number;
  activePackages: number;
  inactivePackages: number;
  reloadStatus: "published";
};

type ErrorPayload = {
  error?: string;
};

export async function createServerSession(title: string): Promise<ServerSession> {
  return requestJson<ServerSession>("/sessions", {
    body: JSON.stringify({ title }),
    method: "POST"
  });
}

export async function postSessionMessage(
  sessionId: string,
  content: string,
  modelSettings?: ModelSettings | null
): Promise<PostMessageResponse> {
  return requestJson<PostMessageResponse>(`/sessions/${sessionId}/messages`, {
    body: JSON.stringify({
      content,
      ...(modelSettings ? { modelSettings } : {})
    }),
    method: "POST"
  });
}

export async function testModelConnection(
  settings: ModelSettings
): Promise<ModelConnectionTestResponse> {
  return requestJson<ModelConnectionTestResponse>("/model/test", {
    body: JSON.stringify(settings),
    method: "POST"
  });
}

export async function listDevSkills(): Promise<DevSkillInventory> {
  return requestJson<DevSkillInventory>("/dev/skills", { method: "GET" });
}

export async function validateDevSkills(): Promise<DevSkillInventory> {
  return requestJson<DevSkillInventory>("/dev/skills/validate", {
    method: "POST"
  });
}

export async function reloadDevSkills(): Promise<DevSkillReloadResponse> {
  return requestJson<DevSkillReloadResponse>("/dev/skills/reload", {
    method: "POST"
  });
}

export async function deleteDevSkill(id: string): Promise<DevSkillInventory> {
  return requestJson<DevSkillInventory>(`/dev/skills/${encodeURIComponent(id)}`, {
    method: "DELETE"
  });
}

export function extractAssistantText(response: PostMessageResponse): string {
  const finished = response.events?.find(
    (event) =>
      event.type === "assistant_message_finished" && typeof event.text === "string"
  );
  if (finished?.text) {
    return finished.text;
  }

  const deltaText = response.events
    ?.filter(
      (event) => event.type === "assistant_text_delta" && typeof event.text === "string"
    )
    .map((event) => event.text)
    .join("");
  if (deltaText) {
    return deltaText;
  }

  return response.assistant_message?.content ?? "";
}

async function requestJson<T>(path: string, init: RequestInit): Promise<T> {
  const response = await fetch(`${SERVER_ORIGIN}${path}`, {
    ...init,
    headers: {
      "Content-Type": "application/json",
      ...init.headers
    }
  });
  const payload = await readPayload(response);

  if (!response.ok) {
    const error = getErrorMessage(payload, response);
    throw new Error(error);
  }

  return payload as T;
}

async function readPayload(response: Response): Promise<unknown> {
  const text = await response.text();
  if (!text) {
    return {};
  }

  try {
    return JSON.parse(text);
  } catch {
    return { error: text };
  }
}

function getErrorMessage(payload: unknown, response: Response): string {
  if (isErrorPayload(payload) && payload.error) {
    return payload.error;
  }

  return response.statusText || `HTTP ${response.status}`;
}

function isErrorPayload(payload: unknown): payload is ErrorPayload {
  return typeof payload === "object" && payload !== null && "error" in payload;
}
