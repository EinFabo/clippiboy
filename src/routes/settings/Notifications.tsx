import { useEffect } from "react";
import { useShallow } from "zustand/react/shallow";
import { useEngine } from "@/store";
import { cn } from "@/lib/cn";
import { api, inTauri } from "@/lib/ipc";
import { NOTIFY_EVENTS, type BannerField } from "@/lib/notifyEvents";
import type { SoundConfig, SoundKind, SoundSetting } from "@/lib/types";
import { Card, SectionTitle } from "@/components/ui/Card";
import { Segmented, Select, Slider, Toggle } from "@/components/ui/Controls";
import { bannerSummary, soundsSummary } from "@/lib/settingsSummary";
import { CornerPad, FOLLOW, Row, SettingsGroup, UNKNOWN, screenChoice } from "./shared";

const percent = (value: number) => `${Math.round(value * 100)} %`;

/** The select's value for Windows' default device. */
const DEFAULT = "";

/** Play button, name, volume, Banner, Sound — the header row uses the same columns. */
const GRID =
  "grid grid-cols-[30px_minmax(120px,190px)_minmax(0,1fr)_46px_64px_64px] items-center gap-3.5 px-5";

/** Banner, sounds and the friends' notices as one list: a few general rows, then one row per event. */
export function NotificationsTab() {
  const { config, targets, patchConfig, devices, refreshSources } = useEngine(
    useShallow((s) => ({
      config: s.config,
      targets: s.targets,
      patchConfig: s.patchConfig,
      devices: s.devices,
      refreshSources: s.refreshSources,
    })),
  );
  const { overlay, sounds, friends } = config;
  const monitors = targets.filter((t) => t.kind === "monitor");

  // A headset plugged in since start shows up without a restart.
  useEffect(() => {
    void refreshSources();
  }, [refreshSources]);

  const patchOverlay = (patch: Partial<typeof overlay>) =>
    patchConfig({ overlay: { ...overlay, ...patch } });
  const patch = (change: Partial<SoundConfig>) => patchConfig({ sounds: { ...sounds, ...change } });
  const patchSound = (kind: SoundKind, change: Partial<SoundSetting>) =>
    patch({ [kind]: { ...sounds[kind], ...change } });
  // At the level it would play at, so turning a slider and listening go together.
  const listen = (kind: SoundKind) => {
    if (inTauri) void api.previewSound(kind, sounds.volume * sounds[kind].volume);
  };

  // Asked the same way as the console's screen — see `screenChoice`.
  const banner = screenChoice(monitors, {
    monitor: overlay.monitor,
    stableId: overlay.monitorStableId,
    follow: overlay.followActiveScreen,
  });

  const outputs = devices.filter((d) => d.kind === "output");
  const deviceOptions = [
    { value: DEFAULT, label: "Windows default" },
    ...outputs.map((d) => ({ value: d.id, label: d.name })),
  ];
  if (sounds.device !== null && !outputs.some((d) => d.id === sounds.device)) {
    deviceOptions.push({
      value: sounds.device,
      label: `${sounds.deviceName ?? "Unknown device"} (not connected)`,
    });
  }

  const screenOptions = [
    // Only there while nothing matches, so the dropdown never shows
    // a screen that is not the one actually in use.
    ...(banner.value === null
      ? [
          {
            value: UNKNOWN,
            label: banner.unplugged ? `${overlay.monitor} · not connected` : "Primary screen",
          },
        ]
      : []),
    ...monitors.map((monitor) => ({
      value: monitor.id,
      label: monitor.isPrimary ? `${monitor.title} · primary` : monitor.title,
    })),
    { value: FOLLOW, label: "Follows the game" },
  ];
  const screenLabel =
    screenOptions.find((o) => o.value === (banner.value ?? UNKNOWN))?.label ?? "Primary screen";

  const bannerOn = (field: BannerField) =>
    field.scope === "overlay" ? overlay[field.key] : friends[field.key];
  const setBanner = (field: BannerField, value: boolean) =>
    field.scope === "overlay"
      ? patchOverlay({ [field.key]: value })
      : patchConfig({ friends: { ...friends, [field.key]: value } });

  return (
    <div className="space-y-8">
      <section>
        <SectionTitle title="Notifications" />
        <Card className="divide-y divide-line">
          <SettingsGroup
            title="Banner over the game"
            storageKey="notifications.banner"
            summary={bannerSummary(screenLabel, overlay.corner, overlay.durationMs)}
            master={{ checked: overlay.enabled, onChange: (enabled) => patchOverlay({ enabled }) }}
          >
            <Row
              label="Banner position"
              hint={banner.unplugged ? "That screen is not connected — the banner goes to the primary one" : undefined}
            >
              <div className="flex items-center gap-3.5">
                <Select
                  label="Screen"
                  value={banner.value ?? UNKNOWN}
                  options={screenOptions}
                  onChange={(value) => {
                    if (value === UNKNOWN) return;
                    patchOverlay(
                      value === FOLLOW
                        ? { followActiveScreen: true }
                        : {
                            monitor: value,
                            // Both halves, so the banner still finds this screen
                            // after the device names have been reshuffled.
                            monitorStableId: monitors.find((m) => m.id === value)?.stableId ?? null,
                            followActiveScreen: false,
                          },
                    );
                  }}
                />
                <CornerPad value={overlay.corner} onChange={(corner) => patchOverlay({ corner })} />
              </div>
            </Row>
            <Row label="Duration">
              <Segmented
                value={String(overlay.durationMs)}
                options={[2000, 3500, 5000, 8000].map((ms) => ({
                  key: String(ms),
                  label: `${ms / 1000} s`,
                }))}
                onChange={(key) => patchOverlay({ durationMs: Number(key) })}
              />
            </Row>
            <Row label="REC badge while recording">
              <Toggle checked={overlay.recBadge} onChange={(recBadge) => patchOverlay({ recBadge })} />
            </Row>
          </SettingsGroup>
          <SettingsGroup
            title="Sounds"
            storageKey="notifications.sounds"
            summary={soundsSummary(
              sounds.device === null ? null : (sounds.deviceName ?? "Unknown device"),
              sounds.volume,
            )}
            master={{
              checked: sounds.enabled,
              onChange: (enabled) => patch({ enabled }),
              label: "Play sounds",
            }}
          >
            <Row label="Sound output">
              <div className="flex items-center gap-4">
                <Select
                  label="Output device"
                  value={sounds.device ?? DEFAULT}
                  options={deviceOptions}
                  onChange={(id) =>
                    patch(
                      id === DEFAULT
                        ? { device: null, deviceName: null }
                        : {
                            device: id,
                            deviceName: outputs.find((d) => d.id === id)?.name ?? sounds.deviceName,
                          },
                    )
                  }
                />
                <div className="w-32">
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
            </Row>
          </SettingsGroup>
          <Row label="Friend banners while I'm playing">
            <Segmented
              value={friends.notifyWhilePlaying ? "all" : "clips"}
              options={[
                { key: "all", label: "All" },
                { key: "clips", label: "Clips only" },
              ]}
              onChange={(key) => patchConfig({ friends: { ...friends, notifyWhilePlaying: key === "all" } })}
            />
          </Row>
        </Card>
        <p className="mt-3 text-xs text-ink-faint">
          The banner cannot appear over a game in exclusive fullscreen — ClippiBoy
          deliberately does not hook into the game process. Borderless fullscreen
          and windowed mode work.
        </p>
      </section>

      <section>
        <SectionTitle title="Per event" />
        <Card className="divide-y divide-line">
          <div className={`${GRID} py-2.5 text-xs font-medium text-ink-muted`}>
            <span />
            <span>Event</span>
            <span>Volume</span>
            <span />
            <span className="text-center">Banner</span>
            <span className="text-center">Sound</span>
          </div>
          {NOTIFY_EVENTS.map(({ label, banner: field, sound, soundNeedsBanner, favoritesOnly }) => {
            const setting = sound ? sounds[sound] : null;
            const gated = !!soundNeedsBanner && !!field && !bannerOn(field);
            const locked = !sounds.enabled || gated;
            const off = locked || !setting?.on;
            return (
              <div key={label} className={`${GRID} py-2`}>
                {sound ? (
                  <button
                    type="button"
                    aria-label={`Listen to ${label}`}
                    title={gated ? "Turn the banner on to hear this one" : "Listen"}
                    onClick={() => listen(sound)}
                    disabled={gated}
                    className="grid size-[30px] shrink-0 place-items-center rounded-pill border border-line
                      bg-elevated text-ink-muted transition-colors hover:bg-hover hover:text-ink
                      disabled:pointer-events-none disabled:opacity-40"
                  >
                    <svg viewBox="0 0 12 12" className="ml-0.5 h-3 w-3" fill="currentColor">
                      <path d="M3 1.8v8.4L10 6z" />
                    </svg>
                  </button>
                ) : (
                  <span className="h-[30px]" />
                )}
                <p className="min-w-0 text-sm font-medium">{label}</p>
                {sound && setting ? (
                  <>
                    <div className={cn("min-w-0", off && !!sounds.enabled && "opacity-40")}>
                      <Slider
                        disabled={locked}
                        label={`Volume of ${label}`}
                        value={Math.round(setting.volume * 100)}
                        min={0}
                        max={100}
                        onChange={(value) => patchSound(sound, { volume: value / 100 })}
                      />
                    </div>
                    <span className="text-right font-mono text-sm text-ink-muted tabular-nums">
                      {percent(setting.volume)}
                    </span>
                  </>
                ) : favoritesOnly && field ? (
                  <>
                    <Segmented
                      className="justify-self-start"
                      value={friends[favoritesOnly] ? "favorites" : "everyone"}
                      options={[
                        { key: "everyone", label: "Everyone" },
                        { key: "favorites", label: "Favorites" },
                      ]}
                      disabled={!bannerOn(field)}
                      onChange={(key) =>
                        patchConfig({ friends: { ...friends, [favoritesOnly]: key === "favorites" } })
                      }
                    />
                    <span />
                  </>
                ) : (
                  <>
                    <span />
                    <span />
                  </>
                )}
                <span className="flex justify-center">
                  {field && (
                    <Toggle
                      checked={bannerOn(field)}
                      disabled={field.scope === "overlay" && !overlay.enabled}
                      onChange={(value) => setBanner(field, value)}
                    />
                  )}
                </span>
                <span className="flex justify-center">
                  {sound && setting && (
                    <Toggle
                      checked={setting.on}
                      disabled={locked}
                      onChange={(on) => patchSound(sound, { on })}
                    />
                  )}
                </span>
              </div>
            );
          })}
        </Card>
      </section>
    </div>
  );
}
