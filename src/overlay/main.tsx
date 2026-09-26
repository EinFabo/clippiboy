import React from "react";
import ReactDOM from "react-dom/client";
import { Overlay } from "./Overlay";
import "../index.css";
import "./overlay.css";
import { installLockdown } from "@/lib/lockdown";
import { installAccent } from "@/lib/accent";

installLockdown();
installAccent();

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <Overlay />
  </React.StrictMode>,
);
