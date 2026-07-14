import { useEffect, useState } from "react";

import { AppearanceProvider } from "./appearance/AppearanceProvider";
import { I18nProvider } from "./i18n/I18nProvider";
import {
  OwnerPolicy,
  canInspectOwnerSkills,
  getOwnerPolicy
} from "./ownerBridge";
import { Chat } from "./screens/Chat";
import { DeveloperTools } from "./screens/DeveloperTools";
import { OwnerSkills } from "./screens/OwnerSkills";
import { Settings } from "./screens/Settings";
import { Accounts } from "./screens/Accounts";
import { Memory } from "./screens/Memory";
import { FoundationActions } from "./screens/FoundationActions";

type AppView = "chat" | "settings" | "developer" | "owner-skills" | "accounts" | "memory" | "actions";

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

    if (window.location.hash === "#accounts") return "accounts";
    if (window.location.hash === "#memory") return "memory";
    if (window.location.hash === "#actions") return "actions";
  }

  return "chat";
}

export default function App(): JSX.Element {
  return (
    <I18nProvider>
      <AppearanceProvider>
        <AppContent />
      </AppearanceProvider>
    </I18nProvider>
  );
}

function AppContent(): JSX.Element {
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
            onOpenAccounts={() => navigate("accounts")}
            onOpenMemory={() => navigate("memory")}
            onOpenActions={() => navigate("actions")}
            ownerPolicy={ownerPolicy}
          />
        )
      ) : view === "developer" ? (
        <DeveloperTools onBack={() => navigate("settings")} />
      ) : view === "accounts" ? (
        <Accounts onBack={() => navigate("settings")} />
      ) : view === "memory" ? (
        <Memory onBack={() => navigate("settings")} />
      ) : view === "actions" ? (
        <FoundationActions onBack={() => navigate("settings")} />
      ) : view === "settings" ? (
        <Settings
          onBack={() => navigate("chat")}
          onOpenDeveloperTools={() => navigate("developer")}
          onOpenOwnerSkills={() => navigate("owner-skills")}
          onOpenAccounts={() => navigate("accounts")}
          onOpenMemory={() => navigate("memory")}
          onOpenActions={() => navigate("actions")}
          ownerPolicy={ownerPolicy}
        />
      ) : (
        <Chat onOpenSettings={() => navigate("settings")} />
      )}
    </div>
  );
}
