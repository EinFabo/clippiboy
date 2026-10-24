import React from "react";
import ReactDOM from "react-dom/client";
import { Overlay } from "./Overlay";
import "../index.css";
import "./overlay.css";
import { installLockdown } from "@/lib/lockdown";
import { installAccent } from "@/lib/accent";
import { rootErrorOptions, installRejectionLogging } from "@/lib/errorReporting";
import { ErrorBoundary } from "@/components/ErrorBoundary";

installLockdown();
installRejectionLogging();
installAccent();

ReactDOM.createRoot(document.getElementById("root")!, rootErrorOptions).render(
  <React.StrictMode>
    <ErrorBoundary compact>
      <Overlay />
    </ErrorBoundary>
  </React.StrictMode>,
);
