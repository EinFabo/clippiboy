import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { useShallow } from "zustand/react/shallow";
import { useEngine } from "@/store";
import { Card } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { Segmented, Toggle } from "@/components/ui/Controls";
import { Mark } from "@/components/ui/Logo";
import { cn } from "@/lib/cn";
import { TargetCard } from "@/routes/Recording";
import { QuickSetup } from "@/routes/AudioMixer";
import { Hotkeys, keyLabel } from "@/routes/settings/Hotkeys";
import { Row, screenChoice } from "@/routes/settings/shared";

const STEPS = ["welcome", "screen", "audio", "hotkeys", "ready"] as const;

/** Buffer lengths offered in the last step, in seconds. */
const LENGTHS = ["60", "90", "120", "180"] as const;
type Length = (typeof LENGTHS)[number];
const LENGTH_LABELS: Record<Length, string> = {
  "60": "1:00",
  "90": "1:30",
  "120": "2:00",
  "180": "3:00",
};

/**
 * Die Einrichtung beim ersten Start: Bildschirm, Ton, Hotkeys, Puffer.
 *
 * Jeder Schritt benutzt dasselbe Bauteil wie seine Seite in der App und wirkt
 * sofort — wer mittendrin abbricht, behält, was er schon eingestellt hat. Nur
 * der letzte Schritt sammelt erst und schreibt beim „Finish“ in einem Rutsch.
 *
 * Liegt unter der Titelleiste (`top-10`, `z-[45]`), damit das Fenster ziehbar
 * und schließbar bleibt — dieselbe Lehre wie beim Player (Punkt 5).
 */
export function Setup() {
  const {
    ready,
    config,
    targets,
    setupOpen,
    setSetupOpen,
    patchConfig,
    upsertSource,
    refreshTargets,
    refreshSources,
    bufferActive,
    toggleBuffer,
  } = useEngine(
    useShallow((s) => ({
      ready: s.ready,
      config: s.config,
      targets: s.targets,
      setupOpen: s.setupOpen,
      setSetupOpen: s.setSetupOpen,
      patchConfig: s.patchConfig,
      upsertSource: s.upsertSource,
      refreshTargets: s.refreshTargets,
      refreshSources: s.refreshSources,
      bufferActive: s.bufferActive,
      toggleBuffer: s.toggleBuffer,
    })),
  );
  const open = ready && (setupOpen || !config.setupDone);
  const [step, setStep] = useState(0);
  /** Seconds as text — a preset, or a length tuned in the settings. */
  const [length, setLength] = useState<string>("120");
  const [autoStart, setAutoStart] = useState(true);
  const [withWindows, setWithWindows] = useState(false);
  const [busy, setBusy] = useState(false);
  /** The clip length as it was on opening, to tell "left alone" from "changed". */
  const [keptLength, setKeptLength] = useState("");

  // Fresh lists each time it opens: a screen or headset plugged in since start.
  // The choices of the last step start from what is configured now.
  useEffect(() => {
    if (!open) return;
    setStep(0);
    void refreshTargets();
    void refreshSources();
    // What a save really writes — a 3:00 buffer with 1:30 clips is a 1:30 clip.
    const clip = config.buffer.clipSeconds === 0
      ? config.buffer.seconds
      : Math.min(config.buffer.clipSeconds, config.buffer.seconds);
    const current = String(clip);
    // A length tuned in the settings stays as it is until a preset is picked.
    setLength(current);
    setKeptLength(current);
    setAutoStart(config.setupDone ? config.buffer.autoStart : true);
    setWithWindows(config.autoStartWithWindows);
    // Only on opening — not every time the config changes underneath.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  if (!open) return null;

  const name = STEPS[step];
  const last = step === STEPS.length - 1;

  const skip = async () => {
    setSetupOpen(false);
    if (!config.setupDone) await patchConfig({ setupDone: true });
  };

  const finish = async () => {
    setBusy(true);
    try {
      // Left alone, buffer and clip length stay exactly as they were — run
      // again from the settings, the setup must not undo a tuned buffer. A new
      // length is the clip's; the buffer only grows if it is shorter.
      const seconds = Number(length);
      const buffer =
        length === keptLength
          ? { ...config.buffer, autoStart }
          : {
              ...config.buffer,
              autoStart,
              seconds: Math.max(config.buffer.seconds, seconds),
              clipSeconds: seconds >= config.buffer.seconds ? 0 : seconds,
            };
      await patchConfig({
        setupDone: true,
        buffer,
        autoStartWithWindows: withWindows,
      });
      if (!useEngine.getState().bufferActive) await toggleBuffer();
      setSetupOpen(false);
    } finally {
      setBusy(false);
    }
  };

  const monitors = targets.filter((t) => t.kind === "monitor");
  const rec = config.recording;
  const picked =
    rec.targetKind === "monitor"
      ? screenChoice(monitors, {
          monitor: rec.targetId,
          stableId: rec.targetStableId,
          follow: false,
        }).value
      : null;
  const saveKey = config.saveClipHotkey
    ? config.saveClipHotkey.split("+").map(keyLabel).join(" + ")
    : null;

  return createPortal(
    <div data-fullview className="fixed inset-x-0 bottom-0 top-10 z-[45] overflow-y-auto bg-base">
      <div
        className="pointer-events-none absolute inset-x-0 top-0 h-[420px] opacity-90"
        style={{ background: "var(--hero-gradient)" }}
      />
      <div className="relative mx-auto flex min-h-full w-full max-w-[720px] flex-col px-8 py-8">
        <header data-tauri-drag-region className="flex items-center justify-between gap-4">
          <div className="flex items-center gap-1.5" aria-label={`Step ${step + 1} of ${STEPS.length}`}>
            {STEPS.map((s, i) => (
              <span
                key={s}
                className={cn(
                  "h-1.5 rounded-pill transition-all duration-200",
                  i === step ? "w-6 bg-white" : i < step ? "w-1.5 bg-white/60" : "w-1.5 bg-white/20",
                )}
              />
            ))}
          </div>
          {!last && (
            <Button size="sm" variant="ghost" onClick={() => void skip()}>
              Skip setup
            </Button>
          )}
        </header>

        <div key={name} className="cb-rise mt-10 flex-1">
          {name === "welcome" && (
            <div className="flex flex-col items-center pt-8 text-center">
              <Mark className="h-24 w-24" />
              <h1 className="mt-6 text-[28px] font-semibold tracking-tight">Welcome to ClippiBoy</h1>
              <p className="mt-3 max-w-[460px] text-sm text-ink-muted">
                ClippiBoy keeps the last minutes of your game, so you can save the best moment
                after it happened. Four quick steps and you are set.
              </p>
            </div>
          )}

          {name === "screen" && (
            <Step title="Which screen do you play on?" hint="This is the screen ClippiBoy records.">
              <div className="grid grid-cols-2 gap-3">
                {monitors.map((t) => (
                  <TargetCard
                    key={t.id}
                    target={t}
                    picked={picked === t.id}
                    onPick={() =>
                      void patchConfig({
                        recording: {
                          ...rec,
                          targetKind: t.kind,
                          targetId: t.id,
                          targetStableId: t.stableId,
                        },
                      })
                    }
                  />
                ))}
              </div>
              {monitors.length === 0 && (
                <p className="text-xs text-ink-faint">
                  No monitor found — ClippiBoy then takes the primary screen.
                </p>
              )}
            </Step>
          )}

          {name === "audio" && (
            <Step title="What should your clips sound like?" hint="Without an audio source your clips are silent.">
              <QuickSetup
                onAdd={upsertSource}
                onPick={() => setStep(step + 1)}
                pickLabel="Later, on the Audio page"
              />
              {config.sources.length > 0 && (
                <Card className="mt-4 divide-y divide-line">
                  {config.sources.map((source) => (
                    <div key={source.id} className="flex items-center justify-between gap-4 px-5 py-3">
                      <p className="truncate text-sm">{source.label}</p>
                      <span className="shrink-0 text-xs text-ink-faint">
                        {source.enabled ? "Recorded" : "Off"}
                      </span>
                    </div>
                  ))}
                </Card>
              )}
              {config.sources.length > 0 && (
                <p className="mt-3 text-xs text-ink-faint">
                  You can add, mute or separate sources any time on the Audio page.
                </p>
              )}
            </Step>
          )}

          {name === "hotkeys" && (
            <Step title="Your hotkeys" hint="They work everywhere, also in-game. Change any of them now or later.">
              <Hotkeys />
            </Step>
          )}

          {name === "ready" && (
            <Step title="Almost done" hint="How long a clip reaches back, and how ClippiBoy starts.">
              <BufferExplainer
                clip={Number(length)}
                buffer={
                  length === keptLength
                    ? config.buffer.seconds
                    : Math.max(config.buffer.seconds, Number(length))
                }
              />
              <Card className="mt-4 divide-y divide-line">
                <Row
                  label="Clip length"
                  hint={
                    isPreset(length)
                      ? "Saving a clip keeps this much of what just happened"
                      : `Now ${clock(Number(length))}, set in the settings. Saving a clip keeps this much of what just happened`
                  }
                >
                  <Segmented<Length>
                    value={isPreset(length) ? length : null}
                    options={LENGTHS.map((key) => ({ key, label: LENGTH_LABELS[key] }))}
                    onChange={setLength}
                  />
                </Row>
                <Row label="Start the buffer automatically">
                  <Toggle checked={autoStart} onChange={setAutoStart} />
                </Row>
                <Row label="Start with Windows" hint="Starts hidden in the tray">
                  <Toggle checked={withWindows} onChange={setWithWindows} />
                </Row>
              </Card>
              <p className="mt-5 rounded-inner border border-line bg-elevated px-4 py-3 text-sm">
                {saveKey ? (
                  <>
                    Press <span className="font-semibold">{saveKey}</span> in-game to save a clip. A
                    banner confirms it.
                  </>
                ) : (
                  <>Save a clip from the tray or the dashboard — there is no hotkey for it yet.</>
                )}
              </p>
            </Step>
          )}
        </div>

        <footer className="mt-10 flex items-center justify-between">
          <Button
            variant="ghost"
            className={cn(step === 0 && "invisible")}
            onClick={() => setStep(step - 1)}
          >
            Back
          </Button>
          {last ? (
            <Button variant="primary" disabled={busy} onClick={() => void finish()}>
              {busy ? "Starting…" : bufferActive ? "Finish" : "Finish and start buffer"}
            </Button>
          ) : (
            <Button variant="primary" onClick={() => setStep(step + 1)}>
              {step === 0 ? "Set up" : "Next"}
            </Button>
          )}
        </footer>
      </div>
    </div>,
    document.body,
  );
}

function isPreset(length: string): length is Length {
  return (LENGTHS as readonly string[]).includes(length);
}

/** "1:30", "3:00". */
function clock(seconds: number): string {
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

/**
 * What the buffer is, in one sentence and one bar: the last minutes always
 * ready, and the clip the latest part of them.
 */
function BufferExplainer({ clip, buffer }: { clip: number; buffer: number }) {
  const share = Math.min(1, clip / buffer);
  return (
    <Card className="p-5">
      <h3 className="text-sm font-semibold">How the buffer works</h3>
      <p className="mt-1 text-xs text-ink-muted">
        While the buffer runs, ClippiBoy keeps the last {clock(buffer)} of your screen and sound
        in memory — nothing is written to disk. Press save and the last {clock(clip)} of it become
        a clip, so the moment you just saw is never lost. Older footage simply rolls off.
      </p>
      <div className="mt-4">
        <div className="mb-1.5 flex justify-between text-[11px] text-ink-faint tabular-nums">
          <span>{clock(buffer)} ago</span>
          <span>now</span>
        </div>
        <div className="flex h-7 overflow-hidden rounded-inner border border-line bg-elevated">
          <div className="h-full" style={{ width: `${(1 - share) * 100}%` }} />
          <div
            className="grid h-full place-items-center bg-accent/30 text-[11px] font-medium text-ink transition-[width] duration-300"
            style={{ width: `${share * 100}%` }}
          >
            your clip · {clock(clip)}
          </div>
        </div>
        <p className="mt-2 text-[11px] text-ink-faint">
          The buffer length itself can be changed later on the Recording page.
        </p>
      </div>
    </Card>
  );
}

function Step({
  title,
  hint,
  children,
}: {
  title: string;
  hint: string;
  children: React.ReactNode;
}) {
  return (
    <section>
      <h2 className="text-[22px] font-semibold tracking-tight">{title}</h2>
      <p className="mt-1 mb-6 text-sm text-ink-muted">{hint}</p>
      {children}
    </section>
  );
}
