import { useEffect, useState } from "react";

import { Chat } from "./screens/Chat";
import { Sessions } from "./screens/Sessions";

type AppView = "chat" | "sessions";

function getViewFromHash(): AppView {
  if (typeof window !== "undefined" && window.location.hash === "#sessions") {
    return "sessions";
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
    if (typeof window !== "undefined") {
      const nextHash = nextView === "sessions" ? "#sessions" : "";
      if (window.location.hash !== nextHash) {
        window.location.hash = nextHash;
      }
    }
  };

  return (
    <div className="app-root">
      {view === "chat" ? (
        <Chat onNavigate={navigate} />
      ) : (
        <Sessions onNavigate={navigate} />
      )}
    </div>
  );
}
