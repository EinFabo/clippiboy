import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./index.css";
import { installLockdown } from "./lib/lockdown";
import { installAccent } from "./lib/accent";
import { rootErrorOptions, installRejectionLogging } from "./lib/errorReporting";
import { ErrorBoundary } from "./components/ErrorBoundary";

installLockdown();
installRejectionLogging();
installAccent({ turning: true });

ReactDOM.createRoot(document.getElementById("root")!, rootErrorOptions).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>,
);
