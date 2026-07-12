import { useEffect, useState } from "react";

import {
  OwnerPolicy,
  canInspectOwnerSkills,
  getOwnerPolicy
} from "./ownerBridge";
import { Chat } from "./screens/Chat";
import { DeveloperTools } from "./screens/DeveloperTools";
import { OwnerSkills } from "./screens/OwnerSkills";
import { Settings } from "./screens/Settings";

type AppView = "chat" | "settings" | "developer" | "owner-skills";

function getViewFromHash(): AppView {
  if (typeof window !== "undefined") {
    if (window.location.hash === "#developer") {
      return "developer";
    }

    if (window.location.hash === "#owner-skills") {
      return "owner-skills";
    }

    if (window.location.hash === "#settings") {
      return "settings";
    }
  }

  return "chat";
}

export default function App(): JSX.Element {
  const [view, setView] = useState<AppView>(getViewFromHash);
  const [ownerPolicy, setOwnerPolicy] = useState<OwnerPolicy | null>(null);

  useEffect(() => {
    let active = true;
    getOwnerPolicy()
      .then((policy) => {
        if (active) setOwnerPolicy(policy);
      })
      .catch(() => {
        if (active) {
          setOwnerPolicy({ mode: "disabled", actorId: "anonymous", role: "anonymous", grants: [] });
        }
      });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    const syncViewFromHash = () => setView(getViewFromHash());

    window.addEventListener("hashchange", syncViewFromHash);

    return () => window.removeEventListener("hashchange", syncViewFromHash);
  }, []);

  const navigate = (nextView: AppView) => {
    setView(nextView);
    const nextHash = nextView === "chat" ? "" : `#${nextView}`;
    if (typeof window !== "undefined" && window.location.hash !== nextHash) {
      window.location.hash = nextHash;
    }
  };

  useEffect(() => {
    if (view === "owner-skills" && ownerPolicy && !canInspectOwnerSkills(ownerPolicy)) {
      navigate("settings");
    }
  }, [ownerPolicy, view]);

  return (
    <div className="app-root">
      {view === "owner-skills" ? (
        canInspectOwnerSkills(ownerPolicy) ? (
          <OwnerSkills
            onBack={() => navigate("settings")}
            policy={ownerPolicy as OwnerPolicy}
          />
        ) : (
          <Settings
            onBack={() => navigate("chat")}
            onOpenDeveloperTools={() => navigate("developer")}
            onOpenOwnerSkills={() => navigate("owner-skills")}
            ownerPolicy={ownerPolicy}
          />
        )
      ) : view === "developer" ? (
        <DeveloperTools onBack={() => navigate("settings")} />
      ) : view === "settings" ? (
        <Settings
          onBack={() => navigate("chat")}
          onOpenDeveloperTools={() => navigate("developer")}
          onOpenOwnerSkills={() => navigate("owner-skills")}
          ownerPolicy={ownerPolicy}
        />
      ) : (
        <Chat onOpenSettings={() => navigate("settings")} />
      )}
    </div>
  );
}
