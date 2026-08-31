import { useEffect } from "react";
import { useEngine } from "@/store";
import { Button } from "@/components/ui/Button";
import { Card, SectionTitle } from "@/components/ui/Card";
import { SourceTrouble } from "@/components/SourceTrouble";
import type { Route } from "@/components/NavBar";
import { Select, Slider } from "@/components/ui/Controls";
import { formatBufferSeconds } from "@/lib/format";
import { cn } from "@/lib/cn";
import type { CaptureTarget, EncoderId } from "@/lib/types";

/** The usual steps; only what the source supports is offered. */
const HEIGHTS = [720, 1080, 1440, 2160];

/** The usual frame rates, as long as the screen does not reveal its own. */
const FPS = [30, 60, 120];

/**
 * Which frame rates the source really supports.
 *
 * Capturing more frames than the screen puts out does not make any motion
 * smoother — it only produces duplicate frames that cost space. Hence the usual
 * steps up to the refresh rate plus that rate itself: a 165 Hz monitor should
 * offer 165 too. That is how the core computes it in `capture::fps_choices`.
 */
function fpsChoices(source: CaptureTarget | null): number[] {
  const refresh = source?.refreshHz ?? null;
  if (!refresh || refresh < 20) return FPS;
  return [...FPS.filter((fps) => fps < refresh), refresh];
}

/** Snap the configured frame rate onto an offered step — downwards, because
    capturing more than was set would be a surprise. */
function fitFps(fps: number, choices: number[]): number {
  if (choices.includes(fps)) return fps;
  return [...choices].reverse().find((step) => step <= fps) ?? choices[0];
}

/**
 * Capture size for a target height. The width comes from the source's aspect
 * ratio instead of an assumed 16:9, and both values stay even — exactly how the
 * core computes it in `capture::fit_to_target`.
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

/**
 * The **upper** bound on what the buffer takes — picture plus sound.
 *
 * The sound is not an afterthought: every source keeps a ring of its own, so
 * that the assignment to the main mix can still be changed afterwards (see
 * `pipeline::tracks_for`). At 48 kHz in stereo that is 192 kB per second and
 * source. In practice it stays well under this: a source that says nothing
 * costs nothing, and one quiet for a whole buffer length hands its memory back
 * (`TrackRing::push`). Which is exactly the normal state of most of them.
 */
function bufferMegabytes(
  bitrateKbps: number,
  seconds: number,
  sources: number,
): number {
  const video = (bitrateKbps / 8 / 1024) * seconds;
  const audio = (sources * seconds * 48000 * 2 * 2) / 1024 ** 2;
  return Math.round(video + audio);
}

export function Recording({ onNavigate }: { onNavigate: (r: Route) => void }) {
  const { config, targets, encoders, patchConfig, refreshTargets } = useEngine();
  const rec = config.recording;

  // Windows come and go: when ClippiBoy started the game usually was not
  // running yet, so the list is fetched again when the page opens.
  useEffect(() => {
    void refreshTargets();
  }, [refreshTargets]);

  const setRec = (patch: Partial<typeof rec>) =>
    patchConfig({ recording: { ...rec, ...patch } });

  const monitors = targets.filter((t) => t.kind === "monitor");
  const windows = targets.filter((t) => t.kind === "window");

  // With no saved selection the core takes the primary monitor — the UI should
  // show that too, otherwise nothing looks selected.
  const isPicked = (t: CaptureTarget) =>
    rec.targetId === null
      ? rec.targetKind === "monitor" && t.kind === "monitor" && t.isPrimary
      : rec.targetKind === t.kind && rec.targetId === t.id;

  // With no selection the core takes the primary monitor — then its resolution
  // should be the limit here too.
  const source =
    targets.find((t) => t.kind === rec.targetKind && t.id === rec.targetId) ??
    targets.find((t) => t.kind === "monitor" && t.isPrimary) ??
    null;

  // No higher than the source: upscaled it only costs bitrate and adds no
  // detail. The core caps it anyway — but at least the same number stands here.
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
  // The choices as text: hanging the effect off the list itself would re-run it
  // on every render, since it is a new array each time.
  const ratesKey = rates.join();

  // Fit size and frame rate to the source. Switching from a 165 Hz monitor to a
  // 60 Hz second screen would otherwise leave a setting standing that means
  // nothing there — the core straightens it out anyway.
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
        <h1 className="display text-4xl">Recording</h1>
      </header>

      <SourceTrouble onOpenMixer={() => onNavigate("audio")} />

      <section>
        <SectionTitle
          title="Source"
          action={
            <Button size="sm" variant="ghost" onClick={() => void refreshTargets()}>
              Refresh
            </Button>
          }
        />

        <p className="mb-4 text-xs text-ink-faint">
          A change takes effect immediately — if the buffer is running it
          restarts on the new source, and the seconds buffered so far are gone.
        </p>

        <h3 className="mb-2 text-xs font-medium text-ink-muted">Monitors</h3>
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
              No monitor found — ClippiBoy then takes the primary screen.
            </p>
          )}
        </div>

        <h3 className="mb-2 mt-6 text-xs font-medium text-ink-muted">
          Windows
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
              No capturable window open. Start the game and hit "Refresh".
            </p>
          )}
        </div>
      </section>

      <section>
        <SectionTitle title="Quality" />
        <Card className="divide-y divide-line">
          <Row
            label="Resolution"
            hint={source ? `${source.width}×${source.height} available` : undefined}
          >
            <Select
              value={String(rec.height)}
              options={heights.map((h) => {
                const size = fit(h, source);
                return {
                  value: String(h),
                  label:
                    source && h === source.height
                      ? `${size.width} × ${size.height} (source)`
                      : `${size.width} × ${size.height}`,
                };
              })}
              onChange={(v) => setRec(fit(Number(v), source))}
            />
          </Row>
          <Row
            label="Frame rate"
            hint={
              source?.refreshHz
                ? `The source shows ${source.refreshHz} Hz — more frames would only be repeats`
                : "The source's refresh rate is not known"
            }
          >
            <Select
              value={String(rec.fps)}
              options={rates.map((fps) => ({
                value: String(fps),
                label:
                  fps === source?.refreshHz
                    ? `${fps} FPS (source)`
                    : `${fps} FPS`,
              }))}
              onChange={(v) => setRec({ fps: Number(v) })}
            />
          </Row>
          <Row
            label="Bitrate"
            hint={`${Math.round(rec.bitrateKbps / 1000)} Mbit/s — about ${Math.round(
              (rec.bitrateKbps / 8 / 1024) * 60,
            )} MB per minute`}
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
            hint="Hardware encoders keep the CPU free — the fallback is x264"
          >
            <Select
              value={rec.encoder}
              options={encoders.map((e) => ({
                value: e.id as EncoderId,
                label: e.available ? e.name : `${e.name} (not available)`,
              }))}
              onChange={(encoder) => setRec({ encoder })}
            />
          </Row>
          <Row
            label="Keyframe interval"
            hint="Determines how precisely a clip can be trimmed"
          >
            <Select
              value={String(rec.keyframeSeconds)}
              options={[
                { value: "1", label: "1 second" },
                { value: "2", label: "2 seconds" },
                { value: "4", label: "4 seconds" },
              ]}
              onChange={(v) => setRec({ keyframeSeconds: Number(v) })}
            />
          </Row>
        </Card>
      </section>

      <section>
        <SectionTitle title="Replay buffer" />
        <Card className="p-5">
          <div className="flex items-center justify-between">
            <div>
              <p className="text-sm font-medium">Buffer length</p>
              <p className="mt-1 text-xs text-ink-muted">
                {formatBufferSeconds(config.buffer.seconds)} in memory · at most{" "}
                {bufferMegabytes(
                  rec.bitrateKbps,
                  config.buffer.seconds,
                  config.sources.filter((s) => s.enabled).length,
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
              label="Buffer length"
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
        // Clear enough to spot at a glance: an 8 % tint on a dark ground is
        // practically indistinguishable from none.
        picked && "border-accent-bright bg-accent/20",
      )}
    >
      {picked && (
        <span
          className="absolute top-3 right-3 rounded-pill bg-accent px-2.5 py-0.5
            text-[11px] font-medium text-white"
        >
          Selected
        </span>
      )}
      <p
        className={cn("truncate text-sm font-medium", picked && "pr-24")}
        title={target.title}
      >
        {target.title}
      </p>
      <p className="mt-1 text-xs text-ink-muted">
        {target.kind === "monitor" ? "Monitor" : "Window"} · {target.width}×
        {target.height}
        {target.refreshHz ? ` · ${target.refreshHz} Hz` : ""}
        {target.isPrimary && " · primary"}
      </p>
    </Card>
  );
}
