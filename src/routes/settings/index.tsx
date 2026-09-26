import { useRef } from "react";

import { useEngine, type SettingsTab } from "@/store";
import { Card } from "@/components/ui/Card";
import { Segmented } from "@/components/ui/Controls";
import { HotkeysTab } from "./Hotkeys";
import { BehaviourTab } from "./Behaviour";
import { BannerTab } from "./Banner";
import { ConsoleTab } from "./Console";
import { StorageTab } from "./Storage";
import { FriendsTab } from "./Friends";
import { AboutTab } from "./About";
import { ColorsTab } from "./Colors";

/** In order of how often someone comes looking. */
const tabs: Array<{ key: SettingsTab; label: string; page: () => React.ReactNode }> = [
  { key: "hotkeys", label: "Hotkeys", page: HotkeysTab },
  { key: "behaviour", label: "Behaviour", page: BehaviourTab },
  { key: "banner", label: "Banner", page: BannerTab },
  { key: "console", label: "Console", page: ConsoleTab },
  { key: "storage", label: "Storage", page: StorageTab },
  { key: "friends", label: "Friends", page: FriendsTab },
  { key: "about", label: "About", page: AboutTab },
];

const extra = { key: "colors" as const, label: "Colors", page: ColorsTab };

const MARK = "99d15a6e77987125bbd2c42bb6ce0c70fd692cf9e6ce9d915a0cca157fc7d0c4";

async function digest(text: string): Promise<string> {
  const bytes = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return [...new Uint8Array(bytes)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

export function Settings() {
  const tab = useEngine((s) => s.settingsTab);
  const setTab = useEngine((s) => s.setSettingsTab);
  const lastError = useEngine((s) => s.lastError);
  const unlocked = useEngine((s) => s.config.colorsUnlocked);
  const patchConfig = useEngine((s) => s.patchConfig);
  const shown = unlocked ? [...tabs, extra] : tabs;
  const Page = shown.find(({ key }) => key === tab)?.page ?? HotkeysTab;
  const taps = useRef<number[]>([]);

  const tap = (at: number) => {
    taps.current = [...taps.current, at].slice(-5);
    if (unlocked || taps.current.length < 5) return;
    void digest(taps.current.join(",")).then((hash) => {
      if (hash !== MARK) return;
      taps.current = [];
      void patchConfig({ colorsUnlocked: true });
      setTab(extra.key);
    });
  };

  return (
    <div className="space-y-8 pb-12">
      <header className="space-y-6 pt-10">
        <h1 className="display select-none text-4xl">
          {[..."Settings"].map((letter, at) => (
            <span key={at} onClick={() => tap(at)}>
              {letter}
            </span>
          ))}
        </h1>
        <Segmented
          className="w-fit"
          value={tab}
          options={shown.map(({ key, label }) => ({ key, label }))}
          onChange={setTab}
        />
      </header>

      <Page />

      {/* Under every tab: a setting that failed to apply may have been made on
          another one. */}
      {lastError && (
        <Card className="border-live/40 bg-live/8 p-4 text-sm text-live">
          {lastError}
        </Card>
      )}
    </div>
  );
}
