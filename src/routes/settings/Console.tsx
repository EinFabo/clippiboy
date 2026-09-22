import { useShallow } from "zustand/react/shallow";
import { useEngine } from "@/store";
import { Card, SectionTitle } from "@/components/ui/Card";
import { Segmented, Select, Slider, Toggle } from "@/components/ui/Controls";
import { FOLLOW, Row, UNKNOWN, screenChoice } from "./shared";

export function ConsoleTab() {
  // `useShallow`, siehe `SourceTrouble`.
  const { config, targets, patchConfig } = useEngine(
    useShallow((s) => ({
      config: s.config,
      targets: s.targets,
      patchConfig: s.patchConfig,
    })),
  );
  const monitors = targets.filter((t) => t.kind === "monitor");

  // Asked the same way as the banner's screen — see `screenChoice`.
  const consoleScreen = screenChoice(monitors, {
    monitor: config.consoleMonitor,
    stableId: config.consoleMonitorStableId,
    follow: config.consoleFollowActiveScreen,
  });

  return (
    <section>
      <SectionTitle title="Console over the game" />
      <Card className="divide-y divide-line">
        <Row
          label="Open with the hotkey"
          hint={`${config.consoleHotkey} brings up the dock over the game — clips, recording, screenshot. Not over games in exclusive fullscreen.`}
        >
          <Toggle
            checked={config.consoleEnabled}
            onChange={(consoleEnabled) => patchConfig({ consoleEnabled })}
          />
        </Row>
        <Row
          label="Size"
          hint={`${Math.round(config.consoleScale * 100)} % — the console always sits at the bottom, above the task bar`}
        >
          <div className="flex w-56 items-center gap-3">
            <Slider
              label="Size of the console"
              value={config.consoleScale}
              min={0.8}
              max={1.6}
              step={0.1}
              onChange={(consoleScale) => patchConfig({ consoleScale })}
            />
            <span className="w-12 shrink-0 text-right text-sm tabular-nums text-ink-muted">
              {Math.round(config.consoleScale * 100)} %
            </span>
          </div>
        </Row>
        <Row
          label="Screen"
          hint={
            consoleScreen.unplugged
              ? "That screen is not connected — the console opens on the primary one"
              : undefined
          }
        >
          <Select
            label="Screen"
            disabled={!config.consoleEnabled}
            value={consoleScreen.value ?? UNKNOWN}
            options={[
              ...(consoleScreen.value === null
                ? [
                    {
                      value: UNKNOWN,
                      label: consoleScreen.unplugged
                        ? `${config.consoleMonitor} · not connected`
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
              { value: FOLLOW, label: "Follows the focus" },
            ]}
            onChange={(value) => {
              if (value === UNKNOWN) return;
              patchConfig(
                value === FOLLOW
                  ? { consoleFollowActiveScreen: true }
                  : {
                      consoleMonitor: value,
                      consoleMonitorStableId:
                        monitors.find((m) => m.id === value)?.stableId ?? null,
                      consoleFollowActiveScreen: false,
                    },
              );
            }}
          />
        </Row>
        <Row
          label="Shape"
          hint={
            config.consoleStyle === "radial"
              ? "The actions sit in a ring around the buffer, the panel beside it"
              : config.consoleStyle === "pill"
                ? "A round dock — the icons carry the meaning, without labels"
                : "A labelled bar along the bottom edge"
          }
        >
          <Segmented
            value={config.consoleStyle}
            disabled={!config.consoleEnabled}
            options={[
              { key: "dock", label: "Bar" },
              { key: "pill", label: "Pill" },
              { key: "radial", label: "Ring" },
            ]}
            onChange={(consoleStyle) => patchConfig({ consoleStyle })}
          />
        </Row>
        <Row
          label="Violet glow"
          hint="A violet wash rises from the lower corners of the screen while the console is open — the gradient the app's own header sits in. It tints the game underneath, brightest at the bottom."
        >
          <Toggle
            checked={config.consoleGlow}
            disabled={!config.consoleEnabled}
            onChange={(consoleGlow) => patchConfig({ consoleGlow })}
          />
        </Row>
        <Row
          label="Visible in screen recordings"
          hint="Then Discord sees the console in a screen share. While it is open it is also inside any clip saved in that time — ClippiBoy records the whole screen."
        >
          <Toggle
            checked={config.consoleInCapture}
            disabled={!config.consoleEnabled}
            onChange={(consoleInCapture) => patchConfig({ consoleInCapture })}
          />
        </Row>
      </Card>
    </section>
  );
}
