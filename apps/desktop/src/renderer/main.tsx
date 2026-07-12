import React from "react";
import ReactDOM from "react-dom/client";
import { Theme } from "@radix-ui/themes";
import { useEffect, useState } from "react";

import App from "./App";
import "@radix-ui/themes/styles.css";
import "./styles/index.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <DesktopTheme />
  </React.StrictMode>
);

function DesktopTheme(): JSX.Element {
  const [appearance, setAppearance] = useState<"dark" | "light">(() =>
    window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light"
  );

  useEffect(() => {
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const update = () => setAppearance(media.matches ? "dark" : "light");
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, []);

  return (
    <Theme
      accentColor="teal"
      appearance={appearance}
      grayColor="gray"
      radius="small"
      scaling="100%"
    >
      <App />
    </Theme>
  );
}
