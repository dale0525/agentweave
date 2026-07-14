import { EndpointType, ModelSettings } from "./types";

export const initialModelSettings: ModelSettings = {
  apiKey: "",
  baseUrl: "http://127.0.0.1:11434/v1",
  endpointType: "responses",
  modelName: "local-agent-model"
};

export const modelSettingsStorageKey = "generalagent.modelSettings.v1";

export type ModelSettingsSnapshot = {
  apiKeyConfigured: boolean;
  saved: boolean;
  settings: ModelSettings;
};

const endpointTypes: EndpointType[] = ["responses", "chat_completions", "completion"];
let browserSettings: ModelSettings | null = null;

export async function loadModelSettings(): Promise<ModelSettingsSnapshot> {
  const bridge = window.generalAgent?.modelSettings;
  if (bridge) {
    await migrateLegacySettings(bridge);
    return snapshotFromUnknown(await bridge.load());
  }
  return loadBrowserSnapshot();
}

export async function loadSavedModelSettings(): Promise<ModelSettings | null> {
  if (window.generalAgent?.modelSettings) return null;
  const snapshot = loadBrowserSnapshot();
  return snapshot.saved ? browserSettings : null;
}

export async function saveModelSettings(settings: ModelSettings): Promise<ModelSettingsSnapshot> {
  const bridge = window.generalAgent?.modelSettings;
  if (bridge) {
    const payload = {
      baseUrl: settings.baseUrl,
      endpointType: settings.endpointType,
      modelName: settings.modelName,
      ...(settings.apiKey ? { apiKey: settings.apiKey } : {})
    };
    return snapshotFromUnknown(await bridge.save(payload));
  }

  browserSettings = {
    ...settings,
    apiKey: settings.apiKey || browserSettings?.apiKey || ""
  };
  persistBrowserMetadata(browserSettings);
  return {
    apiKeyConfigured: Boolean(browserSettings.apiKey),
    saved: true,
    settings: { ...settings, apiKey: "" }
  };
}

export async function clearSavedModelApiKey(settings: ModelSettings): Promise<ModelSettingsSnapshot> {
  const bridge = window.generalAgent?.modelSettings;
  if (bridge) return snapshotFromUnknown(await bridge.clearApiKey());
  browserSettings = { ...settings, apiKey: "" };
  persistBrowserMetadata(browserSettings);
  return { apiKeyConfigured: false, saved: true, settings: browserSettings };
}

function loadBrowserSnapshot(): ModelSettingsSnapshot {
  const raw = readLocalStorage();
  if (!raw) {
    browserSettings = null;
    return { apiKeyConfigured: false, saved: false, settings: initialModelSettings };
  }
  const parsed = modelSettingsFromUnknown(raw);
  if (!browserSettings) browserSettings = parsed;
  if (parsed.apiKey) {
    browserSettings = parsed;
    persistBrowserMetadata(parsed);
  }
  return {
    apiKeyConfigured: Boolean(browserSettings.apiKey),
    saved: true,
    settings: { ...browserSettings, apiKey: "" }
  };
}

async function migrateLegacySettings(
  bridge: NonNullable<NonNullable<Window["generalAgent"]>["modelSettings"]>
): Promise<void> {
  const legacy = readLocalStorage();
  if (!legacy) return;
  try {
    await bridge.save(modelSettingsFromUnknown(legacy));
    window.localStorage.removeItem(modelSettingsStorageKey);
  } catch {
    // Preserve legacy data until the operating-system credential store is available.
  }
}

function readLocalStorage(): unknown | null {
  try {
    const raw = window.localStorage.getItem(modelSettingsStorageKey);
    return raw ? JSON.parse(raw) : null;
  } catch {
    return null;
  }
}

function persistBrowserMetadata(settings: ModelSettings): void {
  try {
    window.localStorage.setItem(
      modelSettingsStorageKey,
      JSON.stringify({
        baseUrl: settings.baseUrl,
        endpointType: settings.endpointType,
        modelName: settings.modelName
      })
    );
  } catch {
    // Browser development mode keeps the settings in memory when storage is unavailable.
  }
}

function snapshotFromUnknown(value: unknown): ModelSettingsSnapshot {
  if (!value || typeof value !== "object") {
    return { apiKeyConfigured: false, saved: false, settings: initialModelSettings };
  }
  const record = value as Record<string, unknown>;
  return {
    apiKeyConfigured: record.apiKeyConfigured === true,
    saved: record.saved === true,
    settings: {
      apiKey: "",
      baseUrl: typeof record.baseUrl === "string" ? record.baseUrl : initialModelSettings.baseUrl,
      endpointType: isEndpointType(record.endpointType)
        ? record.endpointType
        : initialModelSettings.endpointType,
      modelName: typeof record.modelName === "string" ? record.modelName : initialModelSettings.modelName
    }
  };
}

function modelSettingsFromUnknown(value: unknown): ModelSettings {
  if (typeof value !== "object" || value === null) return initialModelSettings;
  const settings = value as Partial<Record<keyof ModelSettings, unknown>>;
  return {
    apiKey: typeof settings.apiKey === "string" ? settings.apiKey : "",
    baseUrl: typeof settings.baseUrl === "string" ? settings.baseUrl : initialModelSettings.baseUrl,
    endpointType: isEndpointType(settings.endpointType)
      ? settings.endpointType
      : initialModelSettings.endpointType,
    modelName: typeof settings.modelName === "string" ? settings.modelName : initialModelSettings.modelName
  };
}

function isEndpointType(value: unknown): value is EndpointType {
  return endpointTypes.some((endpointType) => endpointType === value);
}
