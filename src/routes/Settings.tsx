import { useCallback, useEffect, useRef, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

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
        <Hotkeys />
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
        <ClipDir />
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

/**
 * Anzeigename einer Taste. Gespeichert wird immer die Schreibweise, die der
 * Shortcut-Parser im Kern versteht — hier steht nur, was auf der Tastatur steht.
 */
const KEY_LABELS: Record<string, string> = {
  Ctrl: "Strg",
  Shift: "Umschalt",
  Super: "Win",
  Escape: "Esc",
  Space: "Leertaste",
  Enter: "Enter",
  ArrowUp: "↑",
  ArrowDown: "↓",
  ArrowLeft: "←",
  ArrowRight: "→",
  PageUp: "Bild ↑",
  PageDown: "Bild ↓",
  PrintScreen: "Druck",
  Delete: "Entf",
  Insert: "Einfg",
  Backspace: "Rück",
  CapsLock: "Feststell",
  ScrollLock: "Rollen",
  NumLock: "Num",
  Pause: "Pause",
  NumpadAdd: "Num +",
  NumpadSubtract: "Num −",
  NumpadMultiply: "Num ×",
  NumpadDivide: "Num ÷",
  NumpadDecimal: "Num ,",
  NumpadEnter: "Num Enter",
  NumpadEqual: "Num =",
};

/** Was auf der Taste steht. Der Ziffernblock folgt einem Muster, der Rest steht
    in {@link KEY_LABELS}; alles andere heißt schon so, wie es heißt. */
function keyLabel(key: string): string {
  const numpad = /^Numpad([0-9])$/.exec(key);
  if (numpad) return `Num ${numpad[1]}`;
  return KEY_LABELS[key] ?? key;
}

/**
 * Tasten, die der Hotkey-Parser im Kern kennt und die nicht schon über ihr
 * Muster erkannt werden (Buchstaben, Ziffern, F-Tasten, Ziffernblock).
 *
 * Was hier fehlt, nimmt der Kern nicht an — etwa die Kontextmenü-Taste oder
 * die kleine `<`-Taste neben der linken Umschalttaste. Die Aufnahme läuft
 * dann einfach weiter, statt eine Belegung anzubieten, die gleich wieder
 * abgelehnt würde.
 */
const KEYS = new Set([
  "Backquote", "Backslash", "BracketLeft", "BracketRight", "Comma", "Equal",
  "Minus", "Period", "Quote", "Semicolon", "Slash",
  "Backspace", "CapsLock", "Enter", "Space", "Tab",
  "Delete", "End", "Home", "Insert", "PageDown", "PageUp",
  "PrintScreen", "ScrollLock", "Pause", "NumLock",
  "ArrowDown", "ArrowLeft", "ArrowRight", "ArrowUp",
  "NumpadAdd", "NumpadDecimal", "NumpadDivide", "NumpadEnter", "NumpadEqual",
  "NumpadMultiply", "NumpadSubtract",
  "AudioVolumeDown", "AudioVolumeUp", "AudioVolumeMute",
  "MediaPlayPause", "MediaStop", "MediaTrackNext", "MediaTrackPrevious",
]);

function supported(code: string): boolean {
  return (
    /^(Key[A-Z]|Digit[0-9]|Numpad[0-9]|F([1-9]|1[0-9]|2[0-4]))$/.test(code) ||
    KEYS.has(code)
  );
}

/**
 * Aus einem Tastendruck die Schreibweise machen, die der Kern annimmt.
 *
 * Zusatztasten sind erlaubt, aber nicht verlangt: Wer `F9` allein belegen
 * will, soll das können — so machen es die anderen Aufnahmeprogramme auch.
 */
function accelerator(event: KeyboardEvent): string | null {
  const mods: string[] = [];
  if (event.ctrlKey) mods.push("Ctrl");
  if (event.altKey) mods.push("Alt");
  if (event.shiftKey) mods.push("Shift");
  if (event.metaKey) mods.push("Super");

  const code = event.code;
  // Eine Zusatztaste allein ist noch keine Belegung — weitertippen lassen.
  if (/^(Control|Alt|Shift|Meta|OS)(Left|Right)$/.test(code)) return null;
  if (!supported(code)) return null;

  // `event.code` heißt schon fast überall so wie im Parser; nur die Buchstaben-
  // und Zifferntasten schreibt man üblicherweise kurz.
  const key = /^Key[A-Z]$/.test(code)
    ? code.slice(3)
    : /^Digit[0-9]$/.test(code)
      ? code.slice(5)
      : code;
  return [...mods, key].join("+");
}

/** Zusatztasten, an denen eine Belegung als „nur im Notfall" erkennbar ist. */
const MODIFIERS = ["Ctrl", "Alt", "Shift", "Super"];

/** Tasten, die in einem Textfeld ohnehin nichts schreiben. */
const HARMLESS =
  /^(F([1-9]|1[0-9]|2[0-4])|PrintScreen|ScrollLock|Pause|NumLock|CapsLock|Insert|AudioVolume|Media)/;

/**
 * Eine Belegung, die beim Tippen dazwischenfunkt: eine einzelne Taste, die
 * auch in einem Textfeld etwas zu suchen hat. `F9` oder `Druck` sind harmlos,
 * ein nacktes `S` ist es nicht.
 */
function risky(value: string): boolean {
  const keys = value.split("+");
  if (keys.some((key) => MODIFIERS.includes(key))) return false;
  return !HARMLESS.test(keys[0] ?? "");
}

function Hotkeys() {
  const { config, setHotkeys } = useEngine();
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const warn =
    risky(config.saveClipHotkey) || risky(config.toggleBufferHotkey);

  const apply = async (saveClip: string, toggleBuffer: string) => {
    setSaving(true);
    setError(null);
    try {
      await setHotkeys(saveClip, toggleBuffer);
    } catch (err) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  };

  return (
    <>
      <Card className="divide-y divide-line">
        <Row label="Clip speichern" hint="Speichert den Inhalt des Replay-Puffers">
          <HotkeyInput
            value={config.saveClipHotkey}
            busy={saving}
            onChange={(value) => apply(value, config.toggleBufferHotkey)}
          />
        </Row>
        <Row label="Puffer an/aus">
          <HotkeyInput
            value={config.toggleBufferHotkey}
            busy={saving}
            onChange={(value) => apply(config.saveClipHotkey, value)}
          />
        </Row>
      </Card>
      <p className="mt-3 text-xs text-ink-faint">
        Anklicken und die gewünschte Taste drücken — mit oder ohne Strg, Alt,
        Shift und Windows-Taste. Escape bricht ab.
      </p>
      {warn && (
        <p className="mt-2 text-xs text-ink-muted">
          Eine einzelne Taste, mit der man auch schreiben kann, gilt überall:
          Sie löst mitten im Chat aus. F-Tasten und Druck sind die ruhigeren
          Plätze.
        </p>
      )}
      {error && <p className="mt-2 text-xs text-live">{error}</p>}
    </>
  );
}

/** Zeigt eine Kombination und nimmt auf Klick eine neue auf. */
function HotkeyInput({
  value,
  busy,
  onChange,
}: {
  value: string;
  busy?: boolean;
  onChange: (value: string) => void;
}) {
  const [recording, setRecording] = useState(false);
  // In der Ereignisbehandlung liegt sonst der Wert vom Anfang der Aufnahme.
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  // Wurde die Aufnahme mit einer neuen Kombination beendet? Dann meldet der
  // Kern die Hotkeys selbst wieder an, und ein `resume` hier käme ihm in die
  // Quere.
  const committed = useRef(false);

  const stop = useCallback(() => setRecording(false), []);

  useEffect(() => {
    if (!recording) return;
    committed.current = false;
    if (inTauri) void api.suspendHotkeys();
    const onKeyDown = (event: KeyboardEvent) => {
      // Solange aufgenommen wird, gehört jeder Anschlag hierher — auch Tab und
      // Enter, die sonst durch die Oberfläche wandern würden.
      event.preventDefault();
      event.stopPropagation();
      if (event.code === "Escape") {
        stop();
        return;
      }
      const next = accelerator(event);
      if (!next) return;
      // Auch eine unveränderte Kombination geht durch den Kern: der meldet
      // dabei die stillgelegten Hotkeys wieder an.
      committed.current = true;
      stop();
      onChangeRef.current(next);
    };
    window.addEventListener("keydown", onKeyDown, true);
    // Klick daneben oder Fensterwechsel beendet die Aufnahme ebenfalls.
    window.addEventListener("mousedown", stop);
    window.addEventListener("blur", stop);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("mousedown", stop);
      window.removeEventListener("blur", stop);
      if (inTauri && !committed.current) void api.resumeHotkeys();
    };
  }, [recording, stop]);

  return (
    <button
      type="button"
      disabled={busy}
      onMouseDown={(event) => {
        // Ohne das würde der eigene Klick die Aufnahme sofort wieder beenden.
        event.stopPropagation();
        setRecording((on) => !on);
      }}
      className={cn(
        "flex items-center gap-1.5 rounded-pill border px-2 py-1.5 transition-colors duration-150",
        "disabled:pointer-events-none disabled:opacity-40",
        recording
          ? "border-accent bg-accent/10"
          : "border-transparent hover:border-line hover:bg-elevated",
      )}
    >
      {recording ? (
        <span className="px-1.5 text-[13px] text-accent">Taste drücken …</span>
      ) : (
        <Hotkey value={value} />
      )}
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
          {keyLabel(key)}
        </kbd>
      ))}
    </div>
  );
}

/** Ordner für neue Clips — auswählen, zurücksetzen, öffnen. */
function ClipDir() {
  const { config, setClipDir } = useEngine();
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // Nur für den Knopf „Zurücksetzen": ohne den Vergleich wüsste die Oberfläche
  // nicht, ob überhaupt etwas zurückzusetzen ist.
  const [fallback, setFallback] = useState<string | null>(null);

  useEffect(() => {
    if (!inTauri) return;
    api.defaultClipDir().then(setFallback).catch(() => {});
  }, []);

  const apply = async (dir: string) => {
    setBusy(true);
    setError(null);
    try {
      await setClipDir(dir);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  const pick = async () => {
    setError(null);
    let picked: string | string[] | null;
    try {
      picked = await openDialog({
        directory: true,
        multiple: false,
        defaultPath: config.clipDir,
        title: "Ordner für neue Clips",
      });
    } catch (err) {
      setError(String(err));
      return;
    }
    // Abbruch im Dialog liefert null.
    if (typeof picked !== "string") return;
    await apply(picked);
  };

  const canReset = fallback !== null && fallback !== config.clipDir;

  return (
    <>
      <Card className="flex items-center justify-between gap-6 p-5">
        <p className="truncate font-mono text-sm text-ink-muted" title={config.clipDir}>
          {config.clipDir}
        </p>
        <div className="flex shrink-0 gap-2">
          {canReset && (
            <Button
              size="sm"
              variant="ghost"
              disabled={busy}
              onClick={() => apply(fallback)}
            >
              Zurücksetzen
            </Button>
          )}
          <Button size="sm" variant="secondary" disabled={!inTauri || busy} onClick={pick}>
            Ändern
          </Button>
        </div>
      </Card>
      <p className="mt-3 text-xs text-ink-faint">
        Gilt für neue Clips. Bereits gespeicherte bleiben liegen, wo sie sind,
        und lassen sich weiter abspielen.
      </p>
      {error && <p className="mt-2 text-xs text-live">{error}</p>}
    </>
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
