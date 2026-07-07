import { Wrench } from "lucide-react";
import { useEffect, useState } from "react";

import { listDevSkills } from "../api";

type SettingsDeveloperToolsProps = {
  onOpenDeveloperTools: () => void;
};

export function SettingsDeveloperTools({
  onOpenDeveloperTools
}: SettingsDeveloperToolsProps): JSX.Element | null {
  const [isAvailable, setIsAvailable] = useState(false);

  useEffect(() => {
    let active = true;

    listDevSkills()
      .then(() => {
        if (active) {
          setIsAvailable(true);
        }
      })
      .catch(() => {
        if (active) {
          setIsAvailable(false);
        }
      });

    return () => {
      active = false;
    };
  }, []);

  if (!isAvailable) {
    return null;
  }

  return (
    <section className="settings-panel" aria-labelledby="settings-developer-title">
      <div className="settings-panel-heading">
        <h2 id="settings-developer-title">Developer tools</h2>
        <p>Manage local skill packages while the development API is enabled.</p>
      </div>
      <button
        className="settings-primary-action settings-developer-action"
        onClick={onOpenDeveloperTools}
        type="button"
      >
        <Wrench aria-hidden="true" size={16} /> Open developer tools
      </button>
    </section>
  );
}
