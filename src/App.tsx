import { useEffect, useState } from "react";
import { useEngine } from "./store";
import { TitleBar } from "./components/TitleBar";
import { NavBar, type Route } from "./components/NavBar";
import { Toasts } from "./components/Toasts";
import { Dashboard } from "./routes/Dashboard";
import { Clips } from "./routes/Clips";
import { AudioMixer } from "./routes/AudioMixer";
import { Recording } from "./routes/Recording";
import { Settings } from "./routes/Settings";

export default function App() {
  const [route, setRoute] = useState<Route>("dashboard");
  const init = useEngine((s) => s.init);

  useEffect(() => {
    void init();
  }, [init]);

  return (
    <div className="relative h-full overflow-hidden bg-base">
      {/* Violetter Verlauf oben — die Signatur des Referenz-Designs */}
      <div
        className="pointer-events-none absolute inset-x-0 top-0 h-[420px] opacity-90"
        style={{ background: "var(--hero-gradient)" }}
      />
      <TitleBar />
      <NavBar route={route} onNavigate={setRoute} />

      <main className="relative h-full overflow-y-auto pt-24 pb-16">
        <div className="mx-auto w-full max-w-[1180px] px-8">
          {route === "dashboard" && <Dashboard onNavigate={setRoute} />}
          {route === "clips" && <Clips />}
          {route === "audio" && <AudioMixer />}
          {route === "recording" && <Recording />}
          {route === "settings" && <Settings />}
        </div>
      </main>
      <Toasts />
    </div>
  );
}
