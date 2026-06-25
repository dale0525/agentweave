import { useState } from "react";

import { EndpointType, ModelSettings } from "../types";

const initialSettings: ModelSettings = {
  apiKey: "",
  baseUrl: "http://127.0.0.1:11434/v1",
  endpointType: "responses",
  modelName: "local-agent-model"
};

const endpointOptions: Array<{ label: string; value: EndpointType }> = [
  { label: "Responses", value: "responses" },
  { label: "Chat Completions", value: "chat_completions" },
  { label: "Completion", value: "completion" }
];

export function SettingsModel(): JSX.Element {
  const [settings, setSettings] = useState<ModelSettings>(initialSettings);

  const updateSetting = <Key extends keyof ModelSettings>(
    key: Key,
    value: ModelSettings[Key]
  ) => {
    setSettings((currentSettings) => ({
      ...currentSettings,
      [key]: value
    }));
  };

  return (
    <section className="settings-panel" aria-labelledby="settings-model-title">
      <div className="settings-panel-heading">
        <h2 id="settings-model-title">Model connection</h2>
        <p>Configure the local OpenAI-compatible endpoint used by chat.</p>
      </div>

      <fieldset className="settings-fieldset">
        <legend>Endpoint type</legend>
        <div className="endpoint-selector">
          {endpointOptions.map((option) => (
            <button
              aria-pressed={settings.endpointType === option.value}
              className="endpoint-option"
              key={option.value}
              onClick={() => updateSetting("endpointType", option.value)}
              type="button"
            >
              {option.label}
            </button>
          ))}
        </div>
      </fieldset>

      <div className="settings-form-grid">
        <label className="settings-field">
          <span>Base URL</span>
          <input
            onChange={(event) => updateSetting("baseUrl", event.target.value)}
            type="url"
            value={settings.baseUrl}
          />
        </label>

        <label className="settings-field">
          <span>API key</span>
          <input
            autoComplete="off"
            onChange={(event) => updateSetting("apiKey", event.target.value)}
            type="password"
            value={settings.apiKey}
          />
        </label>

        <label className="settings-field">
          <span>Model name</span>
          <input
            onChange={(event) => updateSetting("modelName", event.target.value)}
            type="text"
            value={settings.modelName}
          />
        </label>
      </div>

      <div className="settings-actions">
        <button className="settings-primary-action" type="button">
          Test connection
        </button>
        <p className="settings-status">Connection: Not tested</p>
      </div>
    </section>
  );
}
