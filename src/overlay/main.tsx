import React from "react";
import ReactDOM from "react-dom/client";
import { Overlay } from "./Overlay";
import "../index.css";
import "./overlay.css";
import { installLockdown } from "@/lib/lockdown";

installLockdown();

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <Overlay />
  </React.StrictMode>,
);
