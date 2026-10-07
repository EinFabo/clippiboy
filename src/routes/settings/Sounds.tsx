import { useShallow } from "zustand/react/shallow";
import { useEngine } from "@/store";
import { api, inTauri } from "@/lib/ipc";
import type { SoundConfig, SoundKind, SoundSetting } from "@/lib/types";
import { Card, SectionTitle } from "@/components/ui/Card";
import { Slider, Toggle } from "@/components/ui/Controls";
import { Row } from "./shared";

/** In the order they come up while playing. */
const SOUNDS: Array<[SoundKind, string, string]> = [
  ["clipSaved", "Clip saved", "When a clip has been written"],
  ["screenshot", "Screenshot", "When a screenshot has been taken"],
  ["recordingStarted", "Recording started", "When a recording begins"],
  ["recordingSaved", "Recording saved", "When a stopped recording is written"],
  ["bufferOn", "Buffer on", "When the replay buffer starts"],
  ["bufferOff", "Buffer off", "When the replay buffer stops"],
  ["friendOnline", "Friend online", "When a friend comes online"],
  ["clipReceived", "Clip from a friend", "When a friend offers a clip, and when it has arrived"],
  ["error", "Errors", "When something fails"],
];

const percent = (value: number) => `${Math.round(value * 100)} %`;

export function SoundsTab() {
  const { sounds, patchConfig } = useEngine(
    useShallow((s) => ({ sounds: s.config.sounds, patchConfig: s.patchConfig })),
  );
  const patch = (change: Partial<SoundConfig>) => patchConfig({ sounds: { ...sounds, ...change } });
  const patchSound = (kind: SoundKind, change: Partial<SoundSetting>) =>
    patch({ [kind]: { ...sounds[kind], ...change } });
  // At the level it would play at, so turning a slider and listening go together.
  const listen = (kind: SoundKind) => {
    if (inTauri) void api.previewSound(kind, sounds.volume * sounds[kind].volume);
  };

  return (
    <div className="space-y-8">
      <section>
        <SectionTitle title="Sounds" />
        <Card className="divide-y divide-line">
          <Row label="Play sounds" hint="A short tone for each notification below, also while the window is in the tray">
            <Toggle checked={sounds.enabled} onChange={(enabled) => patch({ enabled })} />
          </Row>
          <div className="flex items-center gap-6 p-5">
            <p className="w-40 shrink-0 text-sm font-medium">Volume</p>
            <div className="min-w-0 flex-1">
              <Slider
                label="Volume of all sounds"
                value={Math.round(sounds.volume * 100)}
                min={0}
                max={100}
                onChange={(value) => patch({ volume: value / 100 })}
              />
            </div>
            <span className="w-12 shrink-0 text-right font-mono text-sm text-ink-muted tabular-nums">
              {percent(sounds.volume)}
            </span>
          </div>
        </Card>
      </section>

      <section>
        <SectionTitle title="Per notification" />
        <Card className={sounds.enabled ? "divide-y divide-line" : "divide-y divide-line opacity-50"}>
          {SOUNDS.map(([kind, label, hint]) => {
            const setting = sounds[kind];
            const off = !sounds.enabled || !setting.on;
            return (
              <div key={kind} className="flex items-center gap-4 p-5">
                <button
                  type="button"
                  aria-label={`Listen to ${label}`}
                  title="Listen"
                  onClick={() => listen(kind)}
                  className="grid h-9 w-9 shrink-0 place-items-center rounded-pill border border-line
                    bg-elevated text-ink-muted transition-colors hover:bg-hover hover:text-ink"
                >
                  <svg viewBox="0 0 12 12" className="ml-0.5 h-3 w-3" fill="currentColor">
                    <path d="M3 1.8v8.4L10 6z" />
                  </svg>
                </button>
                <div className="w-48 min-w-0 shrink-0">
                  <p className="text-sm font-medium">{label}</p>
                  <p className="mt-1 text-xs text-ink-muted">{hint}</p>
                </div>
                <div className={off ? "min-w-0 flex-1 opacity-40" : "min-w-0 flex-1"}>
                  <Slider
                    label={`Volume of ${label}`}
                    value={Math.round(setting.volume * 100)}
                    min={0}
                    max={100}
                    onChange={(value) => patchSound(kind, { volume: value / 100 })}
                  />
                </div>
                <span className="w-12 shrink-0 text-right font-mono text-sm text-ink-muted tabular-nums">
                  {percent(setting.volume)}
                </span>
                <Toggle
                  checked={setting.on}
                  disabled={!sounds.enabled}
                  onChange={(on) => patchSound(kind, { on })}
                />
              </div>
            );
          })}
        </Card>
      </section>
    </div>
  );
}
