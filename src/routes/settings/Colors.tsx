import { useEffect, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { useEngine } from "@/store";
import { Button } from "@/components/ui/Button";
import { Card, SectionTitle } from "@/components/ui/Card";
import { ColorPicker } from "@/components/ui/ColorPicker";
import { applyAccent } from "@/lib/accent";
import { Row } from "./shared";

/** What everything was designed in — and what "back" goes back to. */
const VIOLET = "#8b5cf6";

const SWATCHES = [VIOLET, "#3b82f6", "#06b6d4", "#22c55e", "#f59e0b", "#ef4444", "#ec4899"];

/**
 * How long the picker may stand still before the choice is written. The field
 * reports every pointer move, and every write goes to disk and out to all three
 * windows; the colour on screen follows at once regardless.
 */
const SETTLE_MS = 150;

export function ColorsTab() {
  const { config, patchConfig, setTab } = useEngine(
    useShallow((s) => ({
      config: s.config,
      patchConfig: s.patchConfig,
      setTab: s.setSettingsTab,
    })),
  );
  const [shown, setShown] = useState(config.accentColor ?? VIOLET);
  const timer = useRef<number | undefined>(undefined);

  // Changed elsewhere — or written back cleaned up by the core.
  useEffect(() => setShown(config.accentColor ?? VIOLET), [config.accentColor]);
  useEffect(() => () => window.clearTimeout(timer.current), []);

  const pick = (color: string) => {
    setShown(color);
    applyAccent(color === VIOLET ? null : color);
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(
      () => void patchConfig({ accentColor: color === VIOLET ? null : color }),
      SETTLE_MS,
    );
  };

  return (
    <section>
      <SectionTitle title="Accent colour" />
      <Card className="divide-y divide-line">
        <div className="space-y-4 p-5">
          <p className="text-sm text-ink-muted">
            One colour for the app, the banner and the console. The lighter and darker steps are
            mixed from it.
          </p>
          <ColorPicker value={shown} swatches={SWATCHES} onChange={pick} />
          {/* What it looks like, before going looking for it. */}
          <div className="flex items-center gap-3">
            <div className="h-10 flex-1 rounded-inner" style={{ background: "var(--hero-gradient)" }} />
            <span className="rounded-pill bg-accent/20 px-3 py-1 text-xs text-accent-bright">
              Accent
            </span>
            <span className="h-3 w-3 rounded-pill bg-accent" />
          </div>
        </div>
        <Row label="Back to violet" hint="The colour ClippiBoy comes in">
          <Button
            size="sm"
            variant="secondary"
            disabled={config.accentColor === null}
            onClick={() => pick(VIOLET)}
          >
            Reset
          </Button>
        </Row>
        <Row label="Hide this tab" hint="The chosen colour stays">
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
