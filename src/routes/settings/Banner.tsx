import { useShallow } from "zustand/react/shallow";
import { useEngine } from "@/store";
import { Card, SectionTitle } from "@/components/ui/Card";
import { Segmented, Select, Toggle } from "@/components/ui/Controls";
import { CornerPad, FOLLOW, Row, UNKNOWN, corners, screenChoice } from "./shared";

export function BannerTab() {
  // `useShallow`, siehe `SourceTrouble`.
  const { config, targets, patchConfig } = useEngine(
    useShallow((s) => ({
      config: s.config,
      targets: s.targets,
      patchConfig: s.patchConfig,
    })),
  );
  const monitors = targets.filter((t) => t.kind === "monitor");

  const patchOverlay = (patch: Partial<typeof config.overlay>) =>
    patchConfig({ overlay: { ...config.overlay, ...patch } });

  // Asked the same way as the console's screen — see `screenChoice`.
  const banner = screenChoice(monitors, {
    monitor: config.overlay.monitor,
    stableId: config.overlay.monitorStableId,
    follow: config.overlay.followActiveScreen,
  });

  return (
    <section>
      <SectionTitle title="Banner over the game" />
      <Card className="divide-y divide-line">
        <Row
          label="Show banner"
          hint="A brief overlay above the game, like Medal or ShadowPlay"
        >
          <Toggle
            checked={config.overlay.enabled}
            onChange={(enabled) => patchOverlay({ enabled })}
          />
        </Row>
        <Row label="Clip saved">
          <Toggle
            checked={config.overlay.onClipSaved}
            disabled={!config.overlay.enabled}
            onChange={(onClipSaved) => patchOverlay({ onClipSaved })}
          />
        </Row>
        <Row label="Buffer on/off">
          <Toggle
            checked={config.overlay.onBufferToggle}
            disabled={!config.overlay.enabled}
            onChange={(onBufferToggle) => patchOverlay({ onBufferToggle })}
          />
        </Row>
        <Row label="Screenshot taken">
          <Toggle
            checked={config.overlay.onScreenshot}
            disabled={!config.overlay.enabled}
            onChange={(onScreenshot) => patchOverlay({ onScreenshot })}
          />
        </Row>
        <Row label="Recording started/saved">
          <Toggle
            checked={config.overlay.onRecording}
            disabled={!config.overlay.enabled}
            onChange={(onRecording) => patchOverlay({ onRecording })}
          />
        </Row>
        <Row
          label="REC badge while recording"
          hint="Stays in the corner for the whole recording, not just at the start"
        >
          <Toggle
            checked={config.overlay.recBadge}
            disabled={!config.overlay.enabled}
            onChange={(recBadge) => patchOverlay({ recBadge })}
          />
        </Row>
        <Row label="Errors">
          <Toggle
            checked={config.overlay.onError}
            disabled={!config.overlay.enabled}
            onChange={(onError) => patchOverlay({ onError })}
          />
        </Row>
        <Row
          label="Screen"
          hint={
            banner.unplugged
              ? "That screen is not connected — the banner goes to the primary one"
              : undefined
          }
        >
          <Select
            label="Screen"
            disabled={!config.overlay.enabled}
            value={banner.value ?? UNKNOWN}
            options={[
              // Only there while nothing matches, so the dropdown never shows
              // a screen that is not the one actually in use.
              ...(banner.value === null
                ? [
                    {
                      value: UNKNOWN,
                      label: banner.unplugged
                        ? `${config.overlay.monitor} · not connected`
                        : "Primary screen",
                    },
                  ]
                : []),
              ...monitors.map((monitor) => ({
                value: monitor.id,
                label: monitor.isPrimary
                  ? `${monitor.title} · primary`
                  : monitor.title,
              })),
              { value: FOLLOW, label: "Follows the game" },
            ]}
            onChange={(value) => {
              if (value === UNKNOWN) return;
              patchOverlay(
                value === FOLLOW
                  ? { followActiveScreen: true }
                  : {
                      monitor: value,
                      // Both halves, so the banner still finds this screen
                      // after the device names have been reshuffled.
                      monitorStableId:
                        monitors.find((m) => m.id === value)?.stableId ?? null,
                      followActiveScreen: false,
                    },
              );
            }}
          />
        </Row>
        <Row
          label="Corner"
          hint={corners.find(([corner]) => corner === config.overlay.corner)?.[1]}
        >
          <CornerPad
            value={config.overlay.corner}
            disabled={!config.overlay.enabled}
            onChange={(corner) => patchOverlay({ corner })}
          />
        </Row>
        <Row label="Duration" hint="Info: Errors always stay at least 6 s">
          <Segmented
            disabled={!config.overlay.enabled}
            value={String(config.overlay.durationMs)}
            options={[2000, 3500, 5000, 8000].map((ms) => ({
              key: String(ms),
              label: `${ms / 1000} s`,
            }))}
            onChange={(key) => patchOverlay({ durationMs: Number(key) })}
          />
        </Row>
      </Card>
      <p className="mt-3 text-xs text-ink-faint">
        The banner cannot appear over a game in exclusive fullscreen — ClippiBoy
        deliberately does not hook into the game process. Borderless fullscreen
        and windowed mode work.
      </p>
    </section>
  );
}
