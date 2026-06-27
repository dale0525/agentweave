import { useState } from "react";

import { testModelConnection } from "../api";
import { loadModelSettings, saveModelSettings } from "../modelSettings";
import { EndpointType, ModelSettings } from "../types";

const endpointOptions: Array<{ label: string; value: EndpointType }> = [
  { label: "Responses", value: "responses" },
  { label: "Chat Completions", value: "chat_completions" },
  { label: "Completion", value: "completion" }
];

export function SettingsModel(): JSX.Element {
  const [settings, setSettings] = useState<ModelSettings>(loadModelSettings);
  const [connectionStatus, setConnectionStatus] = useState("Not tested");
  const [isTestingConnection, setIsTestingConnection] = useState(false);

  const updateSetting = <Key extends keyof ModelSettings>(
    key: Key,
    value: ModelSettings[Key]
  ) => {
    setSettings((currentSettings) => {
      const nextSettings = {
        ...currentSettings,
        [key]: value
      };
      saveModelSettings(nextSettings);
      return nextSettings;
    });
  };

  const testConnection = async () => {
    setIsTestingConnection(true);
    setConnectionStatus("Testing...");

    try {
      const response = await testModelConnection(settings);
      setConnectionStatus(response.message);
    } catch (error) {
      const message = error instanceof Error ? error.message : "Unknown error";
      setConnectionStatus(`Failed: ${message}`);
    } finally {
      setIsTestingConnection(false);
    }
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
        <button
          className="settings-primary-action"
          disabled={isTestingConnection}
          onClick={testConnection}
          type="button"
        >
          Test connection
        </button>
        <p className="settings-status">Connection: {connectionStatus}</p>
      </div>
    </section>
  );
}
