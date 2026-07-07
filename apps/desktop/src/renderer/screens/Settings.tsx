import { ArrowLeft } from "lucide-react";

import { AppIconButton } from "../components/AppIconButton";
import { SettingsDeveloperTools } from "../components/SettingsDeveloperTools";
import { SettingsModel } from "../components/SettingsModel";

type SettingsProps = {
  onBack: () => void;
  onOpenDeveloperTools: () => void;
};

export function Settings({
  onBack,
  onOpenDeveloperTools
}: SettingsProps): JSX.Element {
  return (
    <main className="settings-screen" aria-label="Settings">
      <header className="top-bar settings-top-bar">
        <AppIconButton label="Back to chat" onClick={onBack}>
          <ArrowLeft size={18} aria-hidden="true" />
        </AppIconButton>
        <div className="top-bar-title">
          <h1>Settings</h1>
        </div>
        <span className="top-bar-spacer" aria-hidden="true" />
      </header>
      <div className="settings-shell">
        <SettingsModel />
        <SettingsDeveloperTools onOpenDeveloperTools={onOpenDeveloperTools} />
      </div>
    </main>
  );
}
