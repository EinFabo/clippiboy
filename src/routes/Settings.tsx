import { useEffect, useState } from "react";

import { useEngine } from "@/store";
import { Card, SectionTitle } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { Toggle } from "@/components/ui/Controls";
import { api, events, inTauri } from "@/lib/ipc";
import { cn } from "@/lib/cn";
import type { OverlayCorner, UpdateInfo } from "@/lib/types";

const corners: [OverlayCorner, string][] = [
  ["topLeft", "oben links"],
  ["topRight", "oben rechts"],
  ["bottomLeft", "unten links"],
  ["bottomRight", "unten rechts"],
];

export function Settings() {
  const { config, targets, patchConfig, lastError } = useEngine();
  const monitors = targets.filter((t) => t.kind === "monitor");

  const patchOverlay = (patch: Partial<typeof config.overlay>) =>
    patchConfig({ overlay: { ...config.overlay, ...patch } });

  return (
    <div className="space-y-8 pb-12">
      <header className="pt-10">
        <h1 className="display text-4xl">Einstellungen</h1>
      </header>

      <section>
        <SectionTitle title="Hotkeys" />
        <Card className="divide-y divide-line">
          <Row label="Clip speichern" hint="Speichert den Inhalt des Replay-Puffers">
            <Hotkey value={config.saveClipHotkey} />
          </Row>
          <Row label="Puffer an/aus">
            <Hotkey value={config.toggleBufferHotkey} />
          </Row>
        </Card>
      </section>

      <section>
        <SectionTitle title="Verhalten" />
        <Card className="divide-y divide-line">
          <Row
            label="Puffer automatisch einschalten"
            hint="Ohne das läuft ClippiBoy nur mit, wenn du den Puffer selbst startest"
          >
            <Toggle
              checked={config.buffer.autoStart}
              onChange={(autoStart) =>
                patchConfig({ buffer: { ...config.buffer, autoStart } })
              }
            />
          </Row>
          <Row
            label="Nur im Spiel puffern"
            hint={
              config.buffer.autoStart
                ? "An: der Puffer startet erst, wenn ein Spiel im Vordergrund ist, und stoppt eine halbe Minute nach dem Beenden. Aus: er läuft ab dem Start von ClippiBoy durch."
                : "Wirkt erst, wenn der Puffer automatisch eingeschaltet wird"
            }
          >
            <Toggle
              checked={config.onlyBufferInGame}
              disabled={!config.buffer.autoStart}
              onChange={(onlyBufferInGame) => patchConfig({ onlyBufferInGame })}
            />
          </Row>
          <Row label="Mit Windows starten" hint="Startet versteckt im Tray">
            <Toggle
              checked={config.autoStartWithWindows}
              onChange={(autoStartWithWindows) =>
                patchConfig({ autoStartWithWindows })
              }
            />
          </Row>
        </Card>
      </section>

      <section>
        <SectionTitle title="Banner über dem Spiel" />
        <Card className="divide-y divide-line">
          <Row
            label="Banner anzeigen"
            hint="Kurze Einblendung über dem Spiel, wie bei Medal oder ShadowPlay"
          >
            <Toggle
              checked={config.overlay.enabled}
              onChange={(enabled) => patchOverlay({ enabled })}
            />
          </Row>
          <Row label="Clip gespeichert" hint="Mit Vorschaubild, Spiel und Länge">
            <Toggle
              checked={config.overlay.onClipSaved}
              disabled={!config.overlay.enabled}
              onChange={(onClipSaved) => patchOverlay({ onClipSaved })}
            />
          </Row>
          <Row label="Puffer an/aus" hint="Damit man ohne Fenster weiß, ob aufgenommen wird">
            <Toggle
              checked={config.overlay.onBufferToggle}
              disabled={!config.overlay.enabled}
              onChange={(onBufferToggle) => patchOverlay({ onBufferToggle })}
            />
          </Row>
          <Row label="Fehler" hint="Sonst merkt man beim Spielen nicht, dass nichts aufgenommen wird">
            <Toggle
              checked={config.overlay.onError}
              disabled={!config.overlay.enabled}
              onChange={(onError) => patchOverlay({ onError })}
            />
          </Row>
          <Row
            label="Bildschirm"
            hint="Fest verankert, damit der Banner nicht zwischen Monitoren springt"
          >
            <div className="flex flex-wrap justify-end gap-1.5">
              {monitors.map((monitor) => (
                <ChoiceButton
                  key={monitor.id}
                  active={
                    !config.overlay.followActiveScreen &&
                    (config.overlay.monitor === monitor.id ||
                      (config.overlay.monitor === null && monitor.isPrimary))
                  }
                  disabled={!config.overlay.enabled}
                  onClick={() =>
                    patchOverlay({
                      monitor: monitor.id,
                      followActiveScreen: false,
                    })
                  }
                >
                  {monitor.title.split("—")[0].trim()}
                </ChoiceButton>
              ))}
              <ChoiceButton
                active={config.overlay.followActiveScreen}
                disabled={!config.overlay.enabled}
                onClick={() => patchOverlay({ followActiveScreen: true })}
              >
                folgt dem Spiel
              </ChoiceButton>
            </div>
          </Row>
          <Row label="Ecke" hint="Wo der Banner erscheint">
            <div className="flex gap-1.5">
              {corners.map(([corner, label]) => (
                <ChoiceButton
                  key={corner}
                  active={config.overlay.corner === corner}
                  disabled={!config.overlay.enabled}
                  onClick={() => patchOverlay({ corner })}
                >
                  {label}
                </ChoiceButton>
              ))}
            </div>
          </Row>
          <Row label="Anzeigedauer" hint="Fehler bleiben immer mindestens 6 Sekunden stehen">
            <div className="flex gap-1.5">
              {[2000, 3500, 5000, 8000].map((durationMs) => (
                <ChoiceButton
                  key={durationMs}
                  active={config.overlay.durationMs === durationMs}
                  disabled={!config.overlay.enabled}
                  onClick={() => patchOverlay({ durationMs })}
                >
                  {durationMs / 1000} s
                </ChoiceButton>
              ))}
            </div>
          </Row>
        </Card>
        <p className="mt-3 text-xs text-ink-faint">
          Über einem Spiel im exklusiven Vollbild kann der Banner nicht erscheinen —
          ClippiBoy klinkt sich bewusst nicht in den Spielprozess ein. Im randlosen
          Vollbild und im Fenstermodus funktioniert er.
        </p>
      </section>

      <section>
        <SectionTitle title="Speicherort" />
        <Card className="flex items-center justify-between p-5">
          <p className="truncate font-mono text-sm text-ink-muted">
            {config.clipDir}
          </p>
          <Button size="sm" variant="secondary" disabled={!inTauri}>
            Ändern
          </Button>
        </Card>
      </section>

      <section>
        <SectionTitle title="Version" />
        <Updates />
      </section>

      {lastError && (
        <Card className="border-live/40 bg-live/8 p-4 text-sm text-live">
          {lastError}
        </Card>
      )}

      <p className="text-xs text-ink-faint">
        Windows-Capture über Windows.Graphics.Capture, kein Hooking in
        Spielprozesse.
      </p>
    </div>
  );
}

function Row({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between gap-6 p-5">
      <div className="min-w-0">
        <p className="text-sm font-medium">{label}</p>
        {hint && <p className="mt-1 text-xs text-ink-muted">{hint}</p>}
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  );
}

function ChoiceButton({
  active,
  disabled,
  onClick,
  children,
}: {
  active: boolean;
  disabled?: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      className={cn(
        "h-8 rounded-pill px-3.5 text-[13px] font-medium tabular-nums transition-colors duration-150",
        "disabled:pointer-events-none disabled:opacity-40",
        active
          ? "bg-white text-black"
          : "border border-line bg-elevated text-ink-muted hover:text-ink",
      )}
    >
      {children}
    </button>
  );
}

function Hotkey({ value }: { value: string }) {
  return (
    <div className="flex gap-1.5">
      {value.split("+").map((key) => (
        <kbd
          key={key}
          className="rounded-inner border border-line bg-elevated px-2.5 py-1 font-mono text-xs text-ink-muted"
        >
          {key}
        </kbd>
      ))}
    </div>
  );
}

/** Version, Update-Suche und Installation. */
function Updates() {
  const [version, setVersion] = useState("0.1.0");
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  // `null` heißt: noch nicht gesucht. Sonst der Text unter der Zeile.
  const [state, setState] = useState<"idle" | "checking" | "current" | "failed">(
    "idle",
  );
  const [installing, setInstalling] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!inTauri) return;
    api.appVersion().then(setVersion).catch(() => {});
    // Der Start sucht von sich aus; das Ergebnis kommt als Ereignis herein.
    let unlisten: (() => void) | undefined;
    events.onUpdateAvailable(setUpdate).then((fn) => (unlisten = fn));
    return () => unlisten?.();
  }, []);

  const check = async () => {
    setState("checking");
    setError(null);
    try {
      const found = await api.checkUpdate();
      setUpdate(found);
      setState(found ? "idle" : "current");
    } catch (err) {
      setState("failed");
      setError(String(err));
    }
  };

  const install = async () => {
    setInstalling(true);
    setError(null);
    try {
      // Kommt nicht zurück: Windows beendet die App und startet das Setup.
      await api.installUpdate();
    } catch (err) {
      setInstalling(false);
      setError(String(err));
    }
  };

  return (
    <>
      <Card className="divide-y divide-line">
        <Row
          label={`ClippiBoy ${version}`}
          hint={
            update
              ? `Version ${update.version} ist verfügbar`
              : state === "current"
                ? "Neuester Stand"
                : state === "failed"
                  ? "Die Update-Prüfung ist fehlgeschlagen"
                  : "Updates kommen von GitHub Releases und sind signiert"
          }
        >
          <div className="flex gap-2">
            <Button
              size="sm"
              variant="secondary"
              disabled={!inTauri || state === "checking" || installing}
              onClick={check}
            >
              {state === "checking" ? "Suche …" : "Nach Updates suchen"}
            </Button>
            {update && (
              <Button size="sm" variant="primary" disabled={installing} onClick={install}>
                {installing ? "Installiere …" : `Auf ${update.version} aktualisieren`}
              </Button>
            )}
          </div>
        </Row>
        {update?.notes && (
          <div className="p-5">
            <p className="text-xs whitespace-pre-line text-ink-muted">
              {update.notes}
            </p>
          </div>
        )}
      </Card>
      {update && (
        <p className="mt-3 text-xs text-ink-faint">
          Beim Installieren beendet sich ClippiBoy und das Setup läuft durch —
          ein laufender Puffer wird vorher sauber gestoppt.
        </p>
      )}
      {error && <p className="mt-3 text-xs text-live">{error}</p>}
    </>
  );
}
