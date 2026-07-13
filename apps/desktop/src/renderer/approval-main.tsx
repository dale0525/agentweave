import "@radix-ui/themes/styles.css";
import { Theme } from "@radix-ui/themes";
import React from "react";
import ReactDOM from "react-dom/client";

import { ApprovalSurface } from "./screens/ApprovalSurface";
import "./styles/index.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <Theme accentColor="teal" appearance="inherit" grayColor="gray" radius="small" scaling="100%">
      <ApprovalSurface />
    </Theme>
  </React.StrictMode>
);
