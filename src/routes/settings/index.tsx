import { Fragment, useRef } from "react";

import { useEngine, type SettingsTab } from "@/store";
import { Card } from "@/components/ui/Card";
import { cn } from "@/lib/cn";
import { SLIDE, SlidingIndicator } from "@/components/ui/SlidingIndicator";
import {
  IconBell,
  IconCamera,
  IconConsole,
  IconDisk,
  IconFriends,
  IconInfo,
  IconKey,
  IconPalette,
  IconShare,
  IconSliders,
} from "@/components/icons";
import { HotkeysTab } from "./Hotkeys";
import { GeneralTab } from "./General";
import { NotificationsTab } from "./Notifications";
import { SharingTab } from "./Sharing";
import { ConsoleTab } from "./Console";
import { FacecamTab } from "./Facecam";
import { StorageTab } from "./Storage";
import { FriendsTab } from "./Friends";
import { AboutTab } from "./About";
import { ColorsTab } from "./Colors";

type Entry = {
  key: SettingsTab;
  label: string;
  icon: typeof IconKey;
  page: () => React.ReactNode;
};

/** In order of how often someone comes looking; the gaps are the dividers. */
const groups: Entry[][] = [
  [
    { key: "hotkeys", label: "Hotkeys", icon: IconKey, page: HotkeysTab },
    { key: "general", label: "General", icon: IconSliders, page: GeneralTab },
    { key: "notifications", label: "Notifications", icon: IconBell, page: NotificationsTab },
    { key: "console", label: "Console", icon: IconConsole, page: ConsoleTab },
    { key: "facecam", label: "Camera", icon: IconCamera, page: FacecamTab },
  ],
  [
    { key: "sharing", label: "Sharing", icon: IconShare, page: SharingTab },
    { key: "friends", label: "Friends", icon: IconFriends, page: FriendsTab },
  ],
  [
    { key: "storage", label: "Storage", icon: IconDisk, page: StorageTab },
    { key: "about", label: "About", icon: IconInfo, page: AboutTab },
  ],
];

const extra: Entry = { key: "colors", label: "Colors", icon: IconPalette, page: ColorsTab };

const MARK = "99d15a6e77987125bbd2c42bb6ce0c70fd692cf9e6ce9d915a0cca157fc7d0c4";

async function digest(text: string): Promise<string> {
  const bytes = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return [...new Uint8Array(bytes)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

export function Settings() {
  const tab = useEngine((s) => s.settingsTab);
  const setTab = useEngine((s) => s.setSettingsTab);
  const lastError = useEngine((s) => s.lastError);
  const dismissError = useEngine((s) => s.dismissError);
  const unlocked = useEngine((s) => s.config.colorsUnlocked);
  const patchConfig = useEngine((s) => s.patchConfig);
  const shown = unlocked ? [...groups, [extra]] : groups;
  const Page = shown.flat().find(({ key }) => key === tab)?.page ?? HotkeysTab;
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
      <header className="pt-10">
        <h1 className="display select-none text-4xl">
          {[..."Settings"].map((letter, at) => (
            <span key={at} onClick={() => tap(at)}>
              {letter}
            </span>
          ))}
        </h1>
      </header>

      <div className="grid grid-cols-[210px_minmax(0,1fr)] items-start gap-8">
        {/* The segmented control turned 90 degrees. */}
        <nav
          aria-label="Settings"
          className="relative flex flex-col gap-0.5 rounded-[20px] border border-line bg-elevated p-1"
        >
          <SlidingIndicator activeKey={tab} className="rounded-pill bg-white" />
          {shown.map((group, at) => (
            <Fragment key={at}>
              {at > 0 && <hr className="mx-2.5 my-1 border-0 border-t border-line" />}
              {group.map(({ key, label, icon: Icon }) => (
                <button
                  key={key}
                  {...{ [SLIDE]: key }}
                  onClick={() => setTab(key)}
                  className={cn(
                    "relative flex h-9 items-center gap-2.5 rounded-pill px-3.5 text-[13px] font-medium",
                    "transition-colors duration-150 ease-[var(--ease-out-soft)]",
                    tab === key ? "text-black" : "text-ink-muted hover:text-ink",
                  )}
                >
                  <Icon className="h-[15px] w-[15px] shrink-0" />
                  {label}
                </button>
              ))}
            </Fragment>
          ))}
        </nav>
        <div className="min-w-0 space-y-8">
          <Page />
        </div>
      </div>

      {/* Under every tab: a setting that failed to apply may have been made on
          another one. */}
      {lastError && (
        <Card className="flex items-start justify-between gap-4 border-live/40 bg-live/8 p-4 text-sm text-live">
          <span>{lastError}</span>
          <button
            type="button"
            aria-label="Dismiss"
            className="shrink-0 px-1 leading-none hover:text-white"
            onClick={dismissError}
          >
            ✕
          </button>
        </Card>
      )}
    </div>
  );
}
