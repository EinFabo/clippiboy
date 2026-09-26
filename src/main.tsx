import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./index.css";
import { installLockdown } from "./lib/lockdown";
import { installAccent } from "./lib/accent";

installLockdown();
installAccent({ turning: true });

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
