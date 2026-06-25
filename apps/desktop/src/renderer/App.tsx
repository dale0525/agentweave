import { useEffect, useState } from "react";

import { Chat } from "./screens/Chat";
import { Settings } from "./screens/Settings";

type AppView = "chat" | "settings";

function getViewFromHash(): AppView {
  if (typeof window !== "undefined") {
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
      {view === "settings" ? (
        <Settings onBack={() => navigate("chat")} />
      ) : (
        <Chat onOpenSettings={() => navigate("settings")} />
      )}
    </div>
  );
}
