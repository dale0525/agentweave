import { useState } from "react";

import { Chat } from "./screens/Chat";
import { Sessions } from "./screens/Sessions";

type AppView = "chat" | "sessions";

function getInitialView(): AppView {
  if (typeof window !== "undefined" && window.location.hash === "#sessions") {
    return "sessions";
  }

  return "chat";
}

export default function App(): JSX.Element {
  const [view, setView] = useState<AppView>(getInitialView);

  const navigate = (nextView: AppView) => {
    setView(nextView);
    if (typeof window !== "undefined") {
      window.location.hash = nextView === "sessions" ? "sessions" : "";
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
