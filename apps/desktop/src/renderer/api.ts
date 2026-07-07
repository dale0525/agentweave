import { ModelSettings } from "./types";

const SERVER_ORIGIN = "http://127.0.0.1:49321";

export type ServerSession = {
  id: string;
  title: string;
};

export type RuntimeEvent = {
  type: string;
  text?: string;
  turn_id?: string;
  message?: string;
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
  validation: DevSkillValidation;
};

export type DevSkillInventory = {
  root: string;
  packages: DevSkillPackage[];
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

export async function reloadDevSkills(): Promise<DevSkillInventory> {
  return requestJson<DevSkillInventory>("/dev/skills/reload", {
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
