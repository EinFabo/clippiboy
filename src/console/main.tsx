import React from "react";
import ReactDOM from "react-dom/client";
import { Console } from "./Console";
import "../index.css";
import "./console.css";
import { installLockdown } from "@/lib/lockdown";

installLockdown();

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <Console />
  </React.StrictMode>,
);
