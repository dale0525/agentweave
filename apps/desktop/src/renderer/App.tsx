import { useEffect, useState } from "react";

import { Chat } from "./screens/Chat";
import { DeveloperTools } from "./screens/DeveloperTools";
import { Settings } from "./screens/Settings";

type AppView = "chat" | "settings" | "developer";

function getViewFromHash(): AppView {
  if (typeof window !== "undefined") {
    if (window.location.hash === "#developer") {
      return "developer";
    }

    if (window.location.hash === "#settings") {
      return "settings";
    }
  }

  return "chat";
}

export default function App(): JSX.Element {
  const [view, setView] = useState<AppView>(getViewFromHash);

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

  return (
    <div className="app-root">
      {view === "developer" ? (
        <DeveloperTools onBack={() => navigate("settings")} />
      ) : view === "settings" ? (
        <Settings
          onBack={() => navigate("chat")}
          onOpenDeveloperTools={() => navigate("developer")}
        />
      ) : (
        <Chat onOpenSettings={() => navigate("settings")} />
      )}
    </div>
  );
}
