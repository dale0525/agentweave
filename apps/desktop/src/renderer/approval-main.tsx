import "@radix-ui/themes/styles.css";
import React from "react";
import ReactDOM from "react-dom/client";

import { AppearanceProvider } from "./appearance/AppearanceProvider";
import { I18nProvider } from "./i18n/I18nProvider";
import { ApprovalSurface } from "./screens/ApprovalSurface";
import "./styles/index.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <I18nProvider>
      <AppearanceProvider>
        <ApprovalSurface />
      </AppearanceProvider>
    </I18nProvider>
  </React.StrictMode>
);
