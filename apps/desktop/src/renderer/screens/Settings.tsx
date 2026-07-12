import { ArrowLeft, ShieldCheck } from "lucide-react";

import { AppIconButton } from "../components/AppIconButton";
import { SettingsDeveloperTools } from "../components/SettingsDeveloperTools";
import { SettingsModel } from "../components/SettingsModel";
import { OwnerPolicy, canInspectOwnerSkills } from "../ownerBridge";

type SettingsProps = {
  onBack: () => void;
  onOpenDeveloperTools: () => void;
  onOpenOwnerSkills: () => void;
  ownerPolicy: OwnerPolicy | null;
};

export function Settings({
  onBack,
  onOpenDeveloperTools,
  onOpenOwnerSkills,
  ownerPolicy
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
        {canInspectOwnerSkills(ownerPolicy) ? (
          <section className="settings-panel" aria-labelledby="settings-owner-title">
            <div className="settings-panel-heading">
              <h2 id="settings-owner-title">Owner skills</h2>
              <p>Manage authorized skill packages and revisions.</p>
            </div>
            <button
              className="settings-primary-action settings-developer-action"
              onClick={onOpenOwnerSkills}
              type="button"
            >
              <ShieldCheck aria-hidden="true" size={16} /> Manage skills
            </button>
          </section>
        ) : null}
        <SettingsDeveloperTools onOpenDeveloperTools={onOpenDeveloperTools} />
      </div>
    </main>
  );
}
