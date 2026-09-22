import { useEngine, type SettingsTab } from "@/store";
import { Card } from "@/components/ui/Card";
import { Segmented } from "@/components/ui/Controls";
import { HotkeysTab } from "./Hotkeys";
import { BehaviourTab } from "./Behaviour";
import { BannerTab } from "./Banner";
import { ConsoleTab } from "./Console";
import { StorageTab } from "./Storage";
import { AboutTab } from "./About";

/** In order of how often someone comes looking. */
const tabs: Array<{ key: SettingsTab; label: string; page: () => React.ReactNode }> = [
  { key: "hotkeys", label: "Hotkeys", page: HotkeysTab },
  { key: "behaviour", label: "Behaviour", page: BehaviourTab },
  { key: "banner", label: "Banner", page: BannerTab },
  { key: "console", label: "Console", page: ConsoleTab },
  { key: "storage", label: "Storage", page: StorageTab },
  { key: "about", label: "About", page: AboutTab },
];

export function Settings() {
  const tab = useEngine((s) => s.settingsTab);
  const setTab = useEngine((s) => s.setSettingsTab);
  const lastError = useEngine((s) => s.lastError);
  const Page = tabs.find(({ key }) => key === tab)?.page ?? HotkeysTab;

  return (
    <div className="space-y-8 pb-12">
      <header className="space-y-6 pt-10">
        <h1 className="display text-4xl">Settings</h1>
        <Segmented
          className="w-fit"
          value={tab}
          options={tabs.map(({ key, label }) => ({ key, label }))}
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
