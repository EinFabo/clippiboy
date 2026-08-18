import { useEffect } from "react";
import { useEngine } from "@/store";
import { Button } from "@/components/ui/Button";
import { Card, SectionTitle } from "@/components/ui/Card";
import { Select, Slider } from "@/components/ui/Controls";
import { formatBufferSeconds } from "@/lib/format";
import { cn } from "@/lib/cn";
import type { CaptureTarget, EncoderId } from "@/lib/types";

const RESOLUTIONS = [
  { value: "720", label: "1280 × 720" },
  { value: "1080", label: "1920 × 1080" },
  { value: "1440", label: "2560 × 1440" },
  { value: "2160", label: "3840 × 2160" },
] as const;

const FPS = [
  { value: "30", label: "30 FPS" },
  { value: "60", label: "60 FPS" },
  { value: "120", label: "120 FPS" },
] as const;

export function Recording() {
  const { config, targets, encoders, patchConfig, refreshTargets } = useEngine();
  const rec = config.recording;

  // Fenster kommen und gehen: Beim Start von ClippiBoy lief das Spiel meist
  // noch nicht, also wird die Liste beim Öffnen der Seite neu geholt.
  useEffect(() => {
    void refreshTargets();
  }, [refreshTargets]);

  const setRec = (patch: Partial<typeof rec>) =>
    patchConfig({ recording: { ...rec, ...patch } });

  const monitors = targets.filter((t) => t.kind === "monitor");
  const windows = targets.filter((t) => t.kind === "window");

  // Ohne gespeicherte Auswahl nimmt der Kern den primären Monitor — das soll
  // die Oberfläche auch zeigen, sonst wirkt nichts ausgewählt.
  const isPicked = (t: CaptureTarget) =>
    rec.targetId === null
      ? rec.targetKind === "monitor" && t.kind === "monitor" && t.isPrimary
      : rec.targetKind === t.kind && rec.targetId === t.id;

  return (
    <div className="space-y-8 pb-12">
      <header className="pt-10">
        <h1 className="display text-4xl">Aufnahme</h1>
      </header>

      <section>
        <SectionTitle
          title="Quelle"
          action={
            <Button size="sm" variant="ghost" onClick={() => void refreshTargets()}>
              Aktualisieren
            </Button>
          }
        />

        <p className="mb-4 text-xs text-ink-faint">
          Ein Wechsel greift sofort — läuft der Puffer, startet er mit der neuen
          Quelle neu und die bis dahin gepufferten Sekunden sind weg.
        </p>

        <h3 className="mb-2 text-xs font-medium text-ink-muted">Monitore</h3>
        <div className="grid grid-cols-2 gap-3">
          {monitors.map((t) => (
            <TargetCard
              key={t.id}
              target={t}
              picked={isPicked(t)}
              onPick={() => setRec({ targetKind: t.kind, targetId: t.id })}
            />
          ))}
          {monitors.length === 0 && (
            <p className="text-xs text-ink-faint">
              Kein Monitor gefunden — ClippiBoy nimmt dann den primären
              Bildschirm.
            </p>
          )}
        </div>

        <h3 className="mb-2 mt-6 text-xs font-medium text-ink-muted">
          Fenster
        </h3>
        <div className="grid max-h-72 grid-cols-2 gap-3 overflow-y-auto pr-1">
          {windows.map((t) => (
            <TargetCard
              key={t.id}
              target={t}
              picked={isPicked(t)}
              onPick={() => setRec({ targetKind: t.kind, targetId: t.id })}
            />
          ))}
          {windows.length === 0 && (
            <p className="text-xs text-ink-faint">
              Kein aufnehmbares Fenster offen. Starte das Spiel und tippe auf
              „Aktualisieren“.
            </p>
          )}
        </div>
      </section>

      <section>
        <SectionTitle title="Qualität" />
        <Card className="divide-y divide-line">
          <Row label="Auflösung" hint="Höhe der Aufnahme, Seitenverhältnis folgt der Quelle">
            <Select
              value={String(rec.height)}
              options={RESOLUTIONS.map((r) => ({ value: r.value, label: r.label }))}
              onChange={(v) =>
                setRec({
                  height: Number(v),
                  width: Math.round((Number(v) * 16) / 9),
                })
              }
            />
          </Row>
          <Row label="Bildrate">
            <Select
              value={String(rec.fps)}
              options={FPS.map((f) => ({ value: f.value, label: f.label }))}
              onChange={(v) => setRec({ fps: Number(v) })}
            />
          </Row>
          <Row
            label="Bitrate"
            hint={`${Math.round(rec.bitrateKbps / 1000)} Mbit/s — ca. ${Math.round(
              (rec.bitrateKbps / 8 / 1024) * 60,
            )} MB pro Minute`}
          >
            <div className="w-64">
              <Slider
                label="Bitrate"
                value={rec.bitrateKbps}
                min={5000}
                max={100000}
                step={1000}
                onChange={(bitrateKbps) => setRec({ bitrateKbps })}
              />
            </div>
          </Row>
          <Row
            label="Encoder"
            hint="Hardware-Encoder halten die CPU frei — Fallback ist x264"
          >
            <Select
              value={rec.encoder}
              options={encoders.map((e) => ({
                value: e.id as EncoderId,
                label: e.available ? e.name : `${e.name} (nicht verfügbar)`,
              }))}
              onChange={(encoder) => setRec({ encoder })}
            />
          </Row>
          <Row
            label="Keyframe-Intervall"
            hint="Bestimmt, wie genau ein Clip zugeschnitten werden kann"
          >
            <Select
              value={String(rec.keyframeSeconds)}
              options={[
                { value: "1", label: "1 Sekunde" },
                { value: "2", label: "2 Sekunden" },
                { value: "4", label: "4 Sekunden" },
              ]}
              onChange={(v) => setRec({ keyframeSeconds: Number(v) })}
            />
          </Row>
        </Card>
      </section>

      <section>
        <SectionTitle title="Replay-Puffer" />
        <Card className="p-5">
          <div className="flex items-center justify-between">
            <div>
              <p className="text-sm font-medium">Pufferlänge</p>
              <p className="mt-1 text-xs text-ink-muted">
                {formatBufferSeconds(config.buffer.seconds)} im Arbeitsspeicher ·
                geschätzt{" "}
                {Math.round(
                  (rec.bitrateKbps / 8 / 1024) * config.buffer.seconds,
                )}{" "}
                MB RAM
              </p>
            </div>
            <span className="font-mono text-sm text-ink-muted">
              {formatBufferSeconds(config.buffer.seconds)}
            </span>
          </div>
          <div className="mt-4">
            <Slider
              label="Pufferlänge"
              value={config.buffer.seconds}
              min={15}
              max={600}
              step={15}
              onChange={(seconds) =>
                patchConfig({ buffer: { ...config.buffer, seconds } })
              }
            />
          </div>
        </Card>
      </section>
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

function TargetCard({
  target,
  picked,
  onPick,
}: {
  target: CaptureTarget;
  picked: boolean;
  onPick: () => void;
}) {
  return (
    <Card
      role="button"
      tabIndex={0}
      aria-pressed={picked}
      interactive
      onClick={onPick}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onPick();
        }
      }}
      className={cn(
        "cursor-pointer p-4 text-left",
        picked && "border-accent bg-accent/8",
      )}
    >
      <p className="truncate text-sm font-medium" title={target.title}>
        {target.title}
      </p>
      <p className="mt-1 text-xs text-ink-muted">
        {target.kind === "monitor" ? "Monitor" : "Fenster"} · {target.width}×
        {target.height}
        {target.isPrimary && " · primär"}
      </p>
    </Card>
  );
}
