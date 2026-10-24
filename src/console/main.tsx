import React from "react";
import ReactDOM from "react-dom/client";
import { Console } from "./Console";
import "../index.css";
import "./console.css";
import { installLockdown } from "@/lib/lockdown";
import { installAccent } from "@/lib/accent";
import { rootErrorOptions, installRejectionLogging } from "@/lib/errorReporting";
import { ErrorBoundary } from "@/components/ErrorBoundary";

installLockdown();
installRejectionLogging();
installAccent();

ReactDOM.createRoot(document.getElementById("root")!, rootErrorOptions).render(
  <React.StrictMode>
    <ErrorBoundary>
      <Console />
    </ErrorBoundary>
  </React.StrictMode>,
);
