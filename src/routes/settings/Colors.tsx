import { useEffect, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { useEngine } from "@/store";
import { Button } from "@/components/ui/Button";
import { Card, SectionTitle } from "@/components/ui/Card";
import { ColorPicker } from "@/components/ui/ColorPicker";
import { Segmented } from "@/components/ui/Controls";
import { accentOf, applyAccent } from "@/lib/accent";
import type { AccentMode, AppConfig, RgbSpeed } from "@/lib/types";
import { Row } from "./shared";

/** What everything was designed in — and what "back" goes back to. */
const VIOLET = "#8b5cf6";

const SWATCHES = [VIOLET, "#3b82f6", "#06b6d4", "#22c55e", "#f59e0b", "#ef4444", "#ec4899"];

const MODES: Array<{ key: AccentMode; label: string }> = [
  { key: "solid", label: "Solid" },
  { key: "gradient", label: "Gradient" },
  { key: "rgb", label: "RGB" },
];

const SPEEDS: Array<{ key: RgbSpeed; label: string }> = [
  { key: "slow", label: "Slow" },
  { key: "medium", label: "Medium" },
];

/**
 * How long the picker may stand still before the choice is written. The field
 * reports every pointer move, and every write goes to disk and out to all three
 * windows; the colour on screen follows at once regardless.
 */
const SETTLE_MS = 150;

type AccentPatch = Partial<Pick<AppConfig, "accentMode" | "accentColor" | "accentColor2" | "rgbSpeed">>;

/** The violet is stored as "none" — so a later change of the default reaches it. */
const stored = (color: string) => (color === VIOLET ? null : color);

export function ColorsTab() {
  const { config, patchConfig, setTab } = useEngine(
    useShallow((s) => ({
      config: s.config,
      patchConfig: s.patchConfig,
      setTab: s.setSettingsTab,
    })),
  );
  const [first, setFirst] = useState(config.accentColor ?? VIOLET);
  const [second, setSecond] = useState(config.accentColor2 ?? VIOLET);
  /** RGB picked, the warning standing, not yet confirmed. */
  const [asking, setAsking] = useState(false);
  const timer = useRef<number | undefined>(undefined);
  /**
   * What has been chosen but not written yet. Gathered rather than replaced:
   * a colour picked and a style clicked straight after would otherwise lose the
   * colour — the second change cancels the first one's timer.
   */
  const pending = useRef<AccentPatch>({});

  // Changed elsewhere — or written back cleaned up by the core.
  useEffect(() => setFirst(config.accentColor ?? VIOLET), [config.accentColor]);
  useEffect(() => setSecond(config.accentColor2 ?? VIOLET), [config.accentColor2]);
  /** Write whatever is waiting. */
  const flush = () => {
    window.clearTimeout(timer.current);
    const patch = pending.current;
    pending.current = {};
    if (Object.keys(patch).length) void patchConfig(patch);
  };
  const flushRef = useRef(flush);
  flushRef.current = flush;
  // Leaving the tab within the settle time must not lose the last pick.
  useEffect(() => () => flushRef.current(), []);

  /** On screen now, written once the hand is still. */
  const change = (patch: AccentPatch, settle = true) => {
    pending.current = { ...pending.current, ...patch };
    applyAccent(accentOf({ ...config, ...pending.current }));
    window.clearTimeout(timer.current);
    if (settle) timer.current = window.setTimeout(flush, SETTLE_MS);
    else flush();
  };

  const pickMode = (mode: AccentMode) => {
    if (mode === "rgb" && config.accentMode !== "rgb") {
      setAsking(true);
      return;
    }
    setAsking(false);
    change({ accentMode: mode }, false);
  };

  const mode = config.accentMode;

  return (
    <section>
      <SectionTitle title="Accent colour" />
      <Card className="divide-y divide-line">
        <Row label="Style" hint="One colour, two running into each other, or all of them in turn">
          <Segmented value={asking ? "rgb" : mode} options={MODES} onChange={pickMode} />
        </Row>

        {asking && (
          <div className="space-y-3 border-l-2 border-warn bg-warn/8 p-5">
            <p className="text-sm font-medium text-warn">RGB keeps the app window busy</p>
            <p className="text-xs text-ink-muted">
              While the ClippiBoy window is open, its colours are redrawn several times a second.
              That costs a little CPU and GPU. The console and the banner over a game do not move:
              they take the colour of the moment when they open and keep it. Minimize the window
              while you play if every frame counts.
            </p>
            <div className="flex gap-2">
              <Button
                size="sm"
                variant="primary"
                onClick={() => {
                  setAsking(false);
                  change({ accentMode: "rgb" }, false);
                }}
              >
                Turn on RGB
              </Button>
              <Button size="sm" variant="ghost" onClick={() => setAsking(false)}>
                Cancel
              </Button>
            </div>
          </div>
        )}

        {mode !== "rgb" && (
          <div className="space-y-4 p-5">
            <p className="text-sm text-ink-muted">
              {mode === "gradient"
                ? "The first colour is the bright end — text, borders and buttons wear it too."
                : "One colour for the app, the banner and the console. The lighter and darker steps are mixed from it."}
            </p>
            <ColorPicker
              value={first}
              swatches={SWATCHES}
              onChange={(color) => {
                setFirst(color);
                change({ accentColor: stored(color) });
              }}
            />
            {mode === "gradient" && (
              <>
                <p className="text-sm text-ink-muted">The second colour is where the gradients end.</p>
                <ColorPicker
                  value={second}
                  swatches={SWATCHES}
                  onChange={(color) => {
                    setSecond(color);
                    change({ accentColor2: stored(color) });
                  }}
                />
              </>
            )}
          </div>
        )}

        {mode === "rgb" && (
          <Row label="Speed" hint="How long one round of the colour wheel takes — 30 or 15 seconds">
            <Segmented
              value={config.rgbSpeed}
              options={SPEEDS}
              onChange={(rgbSpeed) => change({ rgbSpeed }, false)}
            />
          </Row>
        )}

        {/* What it looks like, before going looking for it. */}
        <div className="flex items-center gap-3 p-5">
          <div className="h-10 flex-1 rounded-inner" style={{ background: "var(--hero-gradient)" }} />
          <span className="rounded-pill bg-accent/20 px-3 py-1 text-xs text-accent-bright">
            Accent
          </span>
          <span className="h-3 w-3 rounded-pill bg-accent" />
        </div>

        <Row label="Back to violet" hint="The colour ClippiBoy comes in">
          <Button
            size="sm"
            variant="secondary"
            disabled={mode === "solid" && config.accentColor === null}
            onClick={() => {
              setAsking(false);
              setFirst(VIOLET);
              change({ accentMode: "solid", accentColor: null }, false);
            }}
          >
            Reset
          </Button>
        </Row>
        <Row label="Hide this tab" hint="The chosen colours stay">
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              setTab("hotkeys");
              void patchConfig({ colorsUnlocked: false });
            }}
          >
            Hide
          </Button>
        </Row>
      </Card>
    </section>
  );
}
