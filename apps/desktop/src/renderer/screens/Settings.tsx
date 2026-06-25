import * as Tabs from "@radix-ui/react-tabs";
import { ArrowLeft } from "lucide-react";

import { AppIconButton } from "../components/AppIconButton";
import { SettingsModel } from "../components/SettingsModel";
import { SettingsSkills } from "../components/SettingsSkills";

type SettingsProps = {
  onBack: () => void;
};

export function Settings({ onBack }: SettingsProps): JSX.Element {
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
      <Tabs.Root className="settings-shell" defaultValue="model">
        <Tabs.List aria-label="Settings sections" className="settings-tabs">
          <Tabs.Trigger className="settings-tab" value="model">
            Model
          </Tabs.Trigger>
          <Tabs.Trigger className="settings-tab" value="skills">
            Skills
          </Tabs.Trigger>
        </Tabs.List>
        <Tabs.Content className="settings-tab-content" value="model">
          <SettingsModel />
        </Tabs.Content>
        <Tabs.Content className="settings-tab-content" value="skills">
          <SettingsSkills />
        </Tabs.Content>
      </Tabs.Root>
    </main>
  );
}
