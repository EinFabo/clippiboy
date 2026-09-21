import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { useEngine } from "./store";
import { inTauri } from "./lib/ipc";
import type { FocusClip } from "./lib/types";
import { TitleBar } from "./components/TitleBar";
import { NavBar, type Route } from "./components/NavBar";
import { Toasts } from "./components/Toasts";
import { UpdateNotice } from "./components/UpdateNotice";
import { FfmpegNotice } from "./components/FfmpegNotice";
import { TextMenu } from "./components/TextMenu";
import { MenuProvider } from "./components/ui/Menu";
import { Dashboard } from "./routes/Dashboard";
import { Clips } from "./routes/Clips";
import { AudioMixer } from "./routes/AudioMixer";
import { Recording } from "./routes/Recording";
import { Settings } from "./routes/Settings";

export default function App() {
  const [route, setRoute] = useState<Route>("dashboard");
  /** A clip the console handed over, with the second it stood at: the gallery
   *  opens it there on arrival. */
  const [focusClip, setFocusClip] = useState<FocusClip | null>(null);
  const init = useEngine((s) => s.init);

  useEffect(() => {
    void init();
  }, [init]);

  // "Open in the app" from the console over the game: the core brings the
  // window up and says which clip was meant.
  useEffect(() => {
    if (!inTauri) return;
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    listen<FocusClip>("focus-clip", (event) => {
      setRoute("clips");
      setFocusClip(event.payload);
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  return (
    <MenuProvider>
      <div className="relative h-full overflow-hidden bg-base">
        {/* Violet gradient at the top — the signature of the reference design */}
        <div
          className="pointer-events-none absolute inset-x-0 top-0 h-[420px] opacity-90"
          style={{ background: "var(--hero-gradient)" }}
        />
        <TitleBar />
        <NavBar route={route} onNavigate={setRoute} />

        <main className="relative h-full overflow-y-auto pt-24 pb-16">
          {/* Outside the keyed wrapper: it belongs to no page and should not
              replay its entrance on every tab switch. */}
          <div className="mx-auto w-full max-w-[1180px] px-8">
            <FfmpegNotice />
            <UpdateNotice />
          </div>
          {/* Keyed on the route so the rise plays again on every change — the
              same trick the overlay uses to replay its card animation. The
              routes already unmount on a switch, so nothing is lost by it. */}
          <div key={route} className="cb-rise mx-auto w-full max-w-[1180px] px-8">
            {route === "dashboard" && <Dashboard onNavigate={setRoute} />}
            {route === "clips" && (
              <Clips
                onNavigate={setRoute}
                focus={focusClip}
                onFocused={() => setFocusClip(null)}
              />
            )}
            {route === "audio" && <AudioMixer />}
            {route === "recording" && <Recording onNavigate={setRoute} />}
            {route === "settings" && <Settings />}
          </div>
        </main>
        <Toasts />
        {/* Takes the WebView's own menu away and gives text fields one in the
            program's style. */}
        <TextMenu />
      </div>
    </MenuProvider>
  );
}
