import { useEffect } from "react";
import { useEngine } from "@/store";
import { Button } from "@/components/ui/Button";
import { Card, SectionTitle } from "@/components/ui/Card";
import { Select, Slider } from "@/components/ui/Controls";
import { formatBufferSeconds } from "@/lib/format";
import { cn } from "@/lib/cn";
import type { CaptureTarget, EncoderId } from "@/lib/types";

/** Die üblichen Stufen; angeboten wird davon nur, was die Quelle hergibt. */
const HEIGHTS = [720, 1080, 1440, 2160];

/** Die üblichen Bildraten, solange der Bildschirm seine Rate nicht verrät. */
const FPS = [30, 60, 120];

/**
 * Welche Bildraten die Quelle wirklich hergibt.
 *
 * Mehr Bilder aufzunehmen, als der Bildschirm ausgibt, bringt keine Bewegung
 * dazu, flüssiger zu sein — es entstehen nur doppelte Bilder, die Platz
 * kosten. Deshalb die üblichen Stufen bis zur Wiederholrate und die Rate
 * selbst obendrauf: Ein 165-Hz-Monitor soll auch 165 anbieten. So rechnet der
 * Kern in `capture::fps_choices`.
 */
function fpsChoices(source: CaptureTarget | null): number[] {
  const refresh = source?.refreshHz ?? null;
  if (!refresh || refresh < 20) return FPS;
  return [...FPS.filter((fps) => fps < refresh), refresh];
}

/** Die eingestellte Bildrate auf eine angebotene Stufe bringen — nach unten,
    denn mehr aufzunehmen als eingestellt wäre eine Überraschung. */
function fitFps(fps: number, choices: number[]): number {
  if (choices.includes(fps)) return fps;
  return [...choices].reverse().find((step) => step <= fps) ?? choices[0];
}

/**
 * Aufnahmegröße zu einer Zielhöhe. Die Breite kommt aus dem Seitenverhältnis
 * der Quelle statt aus einem angenommenen 16:9, und beide Werte bleiben gerade —
 * genauso rechnet der Kern in `capture::fit_to_target`.
 */
function fit(height: number, source: CaptureTarget | null) {
  const ratio =
    source && source.width > 0 && source.height > 0
      ? source.width / source.height
      : 16 / 9;
  const h = Math.max(2, Math.round(height)) & ~1;
  const w = Math.max(2, Math.round(h * ratio)) & ~1;
  return { width: w, height: h };
}

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

  // Ohne Auswahl nimmt der Kern den primären Monitor — dann soll hier auch
  // dessen Auflösung die Grenze sein.
  const source =
    targets.find((t) => t.kind === rec.targetKind && t.id === rec.targetId) ??
    targets.find((t) => t.kind === "monitor" && t.isPrimary) ??
    null;

  // Höher als die Quelle geht nicht: hochskaliert kostet es nur Bitrate und
  // bringt kein Detail dazu. Der Kern deckelt ohnehin — hier steht dann aber
  // wenigstens dieselbe Zahl.
  const heights = source
    ? [
        ...new Set(
          [...HEIGHTS.filter((h) => h < source.height), source.height].map(
            (h) => fit(h, source).height,
          ),
        ),
      ]
    : HEIGHTS;

  const rates = fpsChoices(source);
  // Die Auswahl als Text: An der Liste selbst hinge der Effekt bei jedem
  // Render neu, sie ist bei jedem Durchlauf ein neues Array.
  const ratesKey = rates.join();

  // Größe und Bildrate an die Quelle angleichen. Wer von einem 165-Hz-Monitor
  // auf einen 60-Hz-Zweitschirm wechselt, hätte sonst eine Einstellung stehen,
  // die dort nichts mehr bedeutet — der Kern rückt sie ohnehin zurecht.
  useEffect(() => {
    if (!source) return;
    const next = fit(Math.min(rec.height, source.height), source);
    const fps = fitFps(rec.fps, rates);
    const sized = next.width !== rec.width || next.height !== rec.height;
    if (sized || fps !== rec.fps) setRec({ ...next, fps });
  }, [
    source?.id,
    source?.width,
    source?.height,
    ratesKey,
    rec.width,
    rec.height,
    rec.fps,
  ]);

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
          <Row
            label="Auflösung"
            hint={
              source
                ? `Seitenverhältnis folgt der Quelle · ${source.width}×${source.height} verfügbar`
                : "Seitenverhältnis folgt der Quelle"
            }
          >
            <Select
              value={String(rec.height)}
              options={heights.map((h) => {
                const size = fit(h, source);
                return {
                  value: String(h),
                  label:
                    source && h === source.height
                      ? `${size.width} × ${size.height} (Quelle)`
                      : `${size.width} × ${size.height}`,
                };
              })}
              onChange={(v) => setRec(fit(Number(v), source))}
            />
          </Row>
          <Row
            label="Bildrate"
            hint={
              source?.refreshHz
                ? `${source.refreshHz} Hz zeigt die Quelle — mehr Bilder wären nur Wiederholungen`
                : "Die Wiederholrate der Quelle ist nicht bekannt"
            }
          >
            <Select
              value={String(rec.fps)}
              options={rates.map((fps) => ({
                value: String(fps),
                label:
                  fps === source?.refreshHz
                    ? `${fps} FPS (Quelle)`
                    : `${fps} FPS`,
              }))}
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
        "relative cursor-pointer p-4 text-left",
        // Deutlich genug, um es beim Überfliegen zu sehen: eine Tönung von 8 %
        // unterscheidet sich auf dunklem Grund praktisch nicht von keiner.
        picked && "border-accent-bright bg-accent/20",
      )}
    >
      {picked && (
        <span
          className="absolute top-3 right-3 rounded-pill bg-accent px-2.5 py-0.5
            text-[11px] font-medium text-white"
        >
          Ausgewählt
        </span>
      )}
      <p
        className={cn("truncate text-sm font-medium", picked && "pr-24")}
        title={target.title}
      >
        {target.title}
      </p>
      <p className="mt-1 text-xs text-ink-muted">
        {target.kind === "monitor" ? "Monitor" : "Fenster"} · {target.width}×
        {target.height}
        {target.refreshHz ? ` · ${target.refreshHz} Hz` : ""}
        {target.isPrimary && " · primär"}
      </p>
    </Card>
  );
}
