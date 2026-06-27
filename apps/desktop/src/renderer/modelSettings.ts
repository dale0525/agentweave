import { EndpointType, ModelSettings } from "./types";

export const initialModelSettings: ModelSettings = {
  apiKey: "",
  baseUrl: "http://127.0.0.1:11434/v1",
  endpointType: "responses",
  modelName: "local-agent-model"
};

export const modelSettingsStorageKey = "generalagent.modelSettings.v1";

const endpointTypes: EndpointType[] = ["responses", "chat_completions", "completion"];

export function loadModelSettings(): ModelSettings {
  return loadSavedModelSettings() ?? initialModelSettings;
}

export function loadSavedModelSettings(): ModelSettings | null {
  if (typeof window === "undefined") {
    return null;
  }

  try {
    const rawSettings = window.localStorage.getItem(modelSettingsStorageKey);
    if (!rawSettings) {
      return null;
    }

    return modelSettingsFromUnknown(JSON.parse(rawSettings));
  } catch {
    return null;
  }
}

export function saveModelSettings(settings: ModelSettings): void {
  if (typeof window === "undefined") {
    return;
  }

  try {
    window.localStorage.setItem(modelSettingsStorageKey, JSON.stringify(settings));
  } catch {
    // Settings should remain editable even when localStorage is unavailable.
  }
}

function modelSettingsFromUnknown(value: unknown): ModelSettings {
  if (typeof value !== "object" || value === null) {
    return initialModelSettings;
  }

  const settings = value as Partial<Record<keyof ModelSettings, unknown>>;

  return {
    apiKey:
      typeof settings.apiKey === "string"
        ? settings.apiKey
        : initialModelSettings.apiKey,
    baseUrl:
      typeof settings.baseUrl === "string"
        ? settings.baseUrl
        : initialModelSettings.baseUrl,
    endpointType: isEndpointType(settings.endpointType)
      ? settings.endpointType
      : initialModelSettings.endpointType,
    modelName:
      typeof settings.modelName === "string"
        ? settings.modelName
        : initialModelSettings.modelName
  };
}

function isEndpointType(value: unknown): value is EndpointType {
  return endpointTypes.some((endpointType) => endpointType === value);
}
