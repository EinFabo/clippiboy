import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { ErrorBoundary } from "@/components/ErrorBoundary";
import { listen } from "@tauri-apps/api/event";
import { useEngine } from "./store";
import { inTauri } from "./lib/ipc";
import { useScrollingFlag } from "./lib/useScrollingFlag";
import { useFriends } from "./lib/friends";
// Pausiert einen Clip, sobald das Fenster in den Tray geht.
import "./lib/hidden";
import type { FocusClip } from "./lib/types";
import { TitleBar } from "./components/TitleBar";
import { NavBar, type Route } from "./components/NavBar";
import { Toasts } from "./components/Toasts";
import { ClipPicker, SharePicker, Transfers } from "./components/Share";
import { UpdateNotice } from "./components/UpdateNotice";
import { FfmpegNotice } from "./components/FfmpegNotice";
import { UsageNotice } from "./components/UsageNotice";
import { DiscordNotice } from "./components/DiscordNotice";
import { TextMenu } from "./components/TextMenu";
import { Setup } from "./components/Setup";
import { MenuProvider } from "./components/ui/Menu";
import { Dashboard } from "./routes/Dashboard";
import { Clips } from "./routes/Clips";
import { AudioMixer } from "./routes/AudioMixer";
import { Recording } from "./routes/Recording";
import { Friends } from "./routes/Friends";
import { Settings } from "./routes/settings";
import { OPEN_SETTINGS_EVENT } from "./lib/settingsLink";
import { stuckSolo } from "./lib/audioRows";

export default function App() {
  const [route, setRoute] = useState<Route>("dashboard");
  /** A clip the console handed over, with the second it stood at: the gallery
   *  opens it there on arrival. */
  const [focusClip, setFocusClip] = useState<FocusClip | null>(null);
  const init = useEngine((s) => s.init);
  const scroller = useRef<HTMLElement>(null);
  useScrollingFlag(scroller);

  // Every page starts at the top, whatever the last one was scrolled to.
  useLayoutEffect(() => {
    scroller.current?.scrollTo(0, 0);
  }, [route]);

  useEffect(() => {
    void init();
  }, [init]);

  // The Audio page has no Solo any more, and the core still honours it: a
  // source saved with it on would silence the others in every recording. Written
  // back as off, once per source.
  const sources = useEngine((s) => s.config.sources);
  const upsertSource = useEngine((s) => s.upsertSource);
  const cleared = useRef(new Set<string>());
  useEffect(() => {
    for (const source of stuckSolo(sources)) {
      if (cleared.current.has(source.id)) continue;
      cleared.current.add(source.id);
      void upsertSource({ ...source, solo: false });
    }
  }, [sources, upsertSource]);

  // `openSettings` from a link anywhere in the window.
  useEffect(() => {
    const open = () => setRoute("settings");
    window.addEventListener(OPEN_SETTINGS_EVENT, open);
    return () => window.removeEventListener(OPEN_SETTINGS_EVENT, open);
  }, []);

  // "Open ClippiBoy" after "Add <name> as friend" on the website: the core
  // brings the window up, the Friends page puts the code into its Add field.
  const addSeq = useFriends((s) => s.add?.seq ?? 0);
  useEffect(() => {
    if (addSeq > 0) setRoute("friends");
  }, [addSeq]);

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

        <main ref={scroller} className="relative h-full overflow-y-auto pt-24 pb-16">
          {/* Outside the keyed wrapper: it belongs to no page and should not
              replay its entrance on every tab switch. */}
          <div className="mx-auto w-full max-w-[1180px] px-8">
            <FfmpegNotice />
            <UpdateNotice />
            <UsageNotice />
            <DiscordNotice onNavigate={setRoute} />
          </div>
          {/* Keyed on the route so the rise plays again on every change — the
              same trick the overlay uses to replay its card animation. The
              routes already unmount on a switch, so nothing is lost by it. */}
          <div key={route} className="cb-rise mx-auto w-full max-w-[1180px] px-8">
            <ErrorBoundary key={route} onBack={() => setRoute("dashboard")}>
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
              {route === "friends" && <Friends />}
              {route === "settings" && <Settings />}
            </ErrorBoundary>
          </div>
        </main>
        <Toasts />
        <Transfers />
        <SharePicker />
        <ClipPicker />
        <Setup />
        {/* Takes the WebView's own menu away and gives text fields one in the
            program's style. */}
        <TextMenu />
      </div>
    </MenuProvider>
  );
}
