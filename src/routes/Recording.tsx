import { useEffect, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { useEngine } from "@/store";
import { Button } from "@/components/ui/Button";
import { Card, SectionTitle } from "@/components/ui/Card";
import { SourceTrouble } from "@/components/SourceTrouble";
import type { Route } from "@/components/NavBar";
import { Segmented, Select, Slider } from "@/components/ui/Controls";
import { Row, SettingsGroup } from "@/routes/settings/shared";
import { formatBufferSeconds } from "@/lib/format";
import {
  BUFFER_PRESETS,
  bufferPreset,
  bufferPresetLabel,
  clipLength,
  clipMegabytes,
  megabytesPerMinute,
  memoryBudgetMb,
  memoryLimitSummary,
  memoryNeededMb,
  nearestStep,
  QUALITY_STEPS,
} from "@/lib/videoPlan";
import { cn } from "@/lib/cn";
import { autoFit, fit, fpsChoices } from "@/lib/fit";
import type { AppConfig, CaptureTarget, EncoderId } from "@/lib/types";

/** The usual steps; only what the source supports is offered. */
const HEIGHTS = [720, 1080, 1440, 2160];

type BufferChoice = "30" | "60" | "120" | "300" | "custom";

export function Recording({ onNavigate }: { onNavigate: (r: Route) => void }) {
  // `useShallow`, siehe `SourceTrouble`.
  const { config, targets, encoders, patchConfig, refreshTargets } = useEngine(
    useShallow((s) => ({
      config: s.config,
      targets: s.targets,
      encoders: s.encoders,
      patchConfig: s.patchConfig,
      refreshTargets: s.refreshTargets,
    })),
  );
  const rec = config.recording;
  /** The buffer sliders while dragged — see there. */
  const [secondsDraft, setSecondsDraft] = useState<number | null>(null);
  const [clipDraft, setClipDraft] = useState<number | null>(null);
  const [memoryDraft, setMemoryDraft] = useState<number | null>(null);
  /** "Custom" was clicked while the length still sits on a step. */
  const [customOpen, setCustomOpen] = useState(false);

  // Windows come and go: when ClippiBoy started the game usually was not
  // running yet, so the list is fetched again when the page opens.
  useEffect(() => {
    void refreshTargets();
  }, [refreshTargets]);

  const setRec = (patch: Partial<typeof rec>) =>
    patchConfig({ recording: { ...rec, ...patch } });

  const monitors = targets.filter((t) => t.kind === "monitor");
  const windows = targets.filter((t) => t.kind === "window");

  // The saved screen, matched the way the core matches it: by the panel's own
  // identity first, and only then by the device name. `\\.\DISPLAY2` is handed
  // out by enumeration order at boot, so after a restart it can name a
  // different panel — going by it alone left every tile unselected and made it
  // look as though the choice had been forgotten.
  const saved =
    rec.targetKind === "monitor"
      ? (targets.find(
          (t) =>
            t.kind === "monitor" &&
            rec.targetStableId !== null &&
            t.stableId === rec.targetStableId,
        ) ??
        targets.find(
          (t) =>
            t.kind === "monitor" && rec.targetId !== null && t.id === rec.targetId,
        ) ??
        null)
      : (targets.find(
          (t) => t.kind === "window" && t.id === rec.targetId,
        ) ?? null);

  // With no saved selection the core takes the primary monitor — the UI should
  // show that too, otherwise nothing looks selected.
  const isPicked = (t: CaptureTarget) =>
    rec.targetId === null && rec.targetStableId === null
      ? rec.targetKind === "monitor" && t.kind === "monitor" && t.isPrimary
      : saved !== null && t.kind === saved.kind && t.id === saved.id;

  // Something was chosen and it is not here. The core keeps buffering on the
  // primary screen rather than stopping — but silently, which is exactly how
  // you end up with a week of clips of the wrong monitor.
  const missing =
    saved === null &&
    (rec.targetId !== null || rec.targetStableId !== null) &&
    targets.length > 0;

  // With no selection the core takes the primary monitor — then its resolution
  // should be the limit here too.
  const source =
    saved ?? targets.find((t) => t.kind === "monitor" && t.isPrimary) ?? null;

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
    const next = autoFit(rec, source, missing);
    if (next) setRec(next);
  }, [
    missing,
    source?.id,
    source?.width,
    source?.height,
    ratesKey,
    rec.width,
    rec.height,
    rec.fps,
  ]);

  const pickTarget = (t: CaptureTarget) =>
    setRec({ targetKind: t.kind, targetId: t.id, targetStableId: t.stableId });

  const quality = nearestStep(rec.quality);
  const encoder = encoders.find((e) => e.id === rec.encoder);
  const encoderMissing = encoder !== undefined && !encoder.available;
  const resolutionLabel = `${source ? fit(rec.height, source).height : rec.height}p`;

  const buffer = config.buffer;
  const bufferShown = secondsDraft ?? buffer.seconds;
  const preset = bufferPreset(buffer.seconds);
  const bufferChoice: BufferChoice =
    preset === null || customOpen ? "custom" : (String(preset) as BufferChoice);
  const writeSeconds = (seconds: number) => {
    if (seconds === buffer.seconds) return;
    void patchConfig({
      buffer: {
        ...buffer,
        seconds,
        // A clip can never outlast the buffer it is cut from.
        clipSeconds: Math.min(buffer.clipSeconds, seconds),
      },
    });
  };

  const camMb = config.facecam.enabled && config.facecam.inClips ? camMemoryMb(config) : 0;
  const limitMb = memoryDraft ?? memoryBudgetMb(rec, buffer);
  const limit = memoryLimitSummary(limitMb, rec.bitrateKbps, buffer.seconds);

  return (
    <div className="space-y-8 pb-12">
      <header className="pt-10">
        <h1 className="display text-4xl">Video</h1>
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
        {/* One grid, monitors first. A long window list scrolls inside it. */}
        <div className="-m-1 grid max-h-80 grid-cols-3 gap-3 overflow-y-auto p-1">
          {missing && (
            <SourceTile
              name={rec.targetId ?? "Screen"}
              meta="Not connected · using primary"
              title="Not connected · recording uses the primary screen"
              picked
              warn
            />
          )}
          {[...monitors, ...windows].map((t) => (
            <SourceTile
              key={t.id}
              name={t.kind === "monitor" ? t.title.replace(/ — \d+×\d+$/, "") : t.title}
              meta={`${t.kind === "window" ? "Window · " : ""}${t.width}×${t.height}${
                t.refreshHz ? ` · ${t.refreshHz} Hz` : ""
              }`}
              chip={t.kind === "monitor" && t.isPrimary ? "Primary" : undefined}
              picked={!missing && isPicked(t)}
              onPick={() => pickTarget(t)}
            />
          ))}
          {targets.length === 0 && (
            <p className="col-span-3 text-xs text-ink-faint">
              No monitor or window found. Start the game and hit "Refresh".
            </p>
          )}
        </div>
      </section>

      <section>
        <SectionTitle title="Quality" />
        <Card className="divide-y divide-line">
          <Row label="Resolution">
            <Segmented<string>
              value={heights.includes(rec.height) ? String(rec.height) : null}
              options={heights.map((h) => ({
                key: String(h),
                label: `${fit(h, source).height}p`,
              }))}
              onChange={(v) => setRec(fit(Number(v), source))}
            />
          </Row>
          <Row label="Frame rate">
            <Segmented<string>
              value={rates.includes(rec.fps) ? String(rec.fps) : null}
              options={rates.map((fps) => ({ key: String(fps), label: String(fps) }))}
              onChange={(v) => setRec({ fps: Number(v) })}
            />
          </Row>
          <Row label="Quality">
            <Segmented<string>
              value={String(quality.value)}
              options={QUALITY_STEPS.map((step) => ({
                key: String(step.value),
                label: step.label,
              }))}
              onChange={(v) => setRec({ quality: Number(v) })}
            />
          </Row>
          <EstimateRow
            left={`${resolutionLabel} · ${rec.fps} fps · ${quality.label}`}
            right={<b>about {megabytesPerMinute(rec.bitrateKbps)} MB per minute</b>}
          />
          <SettingsGroup
            title="Advanced"
            storageKey="video-advanced"
            summary={
              encoderMissing ? (
                <span className="text-warn">
                  {encoder.name} not available · x264 is used
                </span>
              ) : (
                `${encoder?.name ?? rec.encoder} · keyframe every ${rec.keyframeSeconds} s`
              )
            }
          >
            <Row label="Encoder">
              <Select
                value={rec.encoder}
                options={encoders.map((e) => ({
                  value: e.id as EncoderId,
                  label: e.available ? e.name : `${e.name} (not available)`,
                }))}
                onChange={(next) => setRec({ encoder: next })}
              />
            </Row>
            <Row label="Keyframe interval">
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
          </SettingsGroup>
        </Card>
      </section>

      <section>
        <SectionTitle title="Replay buffer" />
        <Card className="divide-y divide-line">
          <Row label="Keep the last">
            <div className="flex flex-col items-end gap-4">
              <Segmented<BufferChoice>
                value={bufferChoice}
                options={[
                  ...BUFFER_PRESETS.map((p) => ({
                    key: String(p) as BufferChoice,
                    label: bufferPresetLabel(p),
                  })),
                  { key: "custom" as BufferChoice, label: "Custom" },
                ]}
                onChange={(key) => {
                  if (key === "custom") {
                    setCustomOpen(true);
                    return;
                  }
                  setCustomOpen(false);
                  writeSeconds(Number(key));
                }}
              />
              {bufferChoice === "custom" && (
                <div className="flex w-full items-center gap-4">
                  {/* Written when the knob is let go. Every step on the way used
                      to reach the core, and a drag that passed below the value
                      it ended on shrank the buffer there — what lay beyond was
                      gone. */}
                  <div className="min-w-0 flex-1">
                    <Slider
                      label="Buffer length"
                      value={bufferShown}
                      min={15}
                      max={600}
                      step={15}
                      onChange={setSecondsDraft}
                      onRelease={() => setSecondsDraft(null)}
                      onCommit={(seconds) => {
                        setSecondsDraft(null);
                        // Landing on a step must not close the slider under the knob.
                        setCustomOpen(true);
                        writeSeconds(seconds);
                      }}
                    />
                  </div>
                  <span className="min-w-14 text-right font-mono text-sm text-ink-muted">
                    {formatBufferSeconds(bufferShown)}
                  </span>
                </div>
              )}
            </div>
          </Row>
          <Row label="A save writes">
            <div className="flex w-80 items-center gap-4">
              <div className="min-w-0 flex-1">
                <Slider
                  label="Clip length"
                  value={clipDraft ?? clipLength(buffer)}
                  min={5}
                  max={buffer.seconds}
                  step={5}
                  onChange={setClipDraft}
                  onRelease={() => setClipDraft(null)}
                  onCommit={(value) => {
                    setClipDraft(null);
                    if (value === clipLength(buffer)) return;
                    void patchConfig({
                      buffer: {
                        ...buffer,
                        // At the top end store 0, not the number: a longer buffer
                        // later then carries the clip length along instead of
                        // leaving it standing where it was.
                        clipSeconds: value >= buffer.seconds ? 0 : value,
                      },
                    });
                  }}
                />
              </div>
              <span className="min-w-14 text-right font-mono text-sm text-ink-muted">
                {formatBufferSeconds(clipDraft ?? clipLength(buffer))}
              </span>
            </div>
          </Row>
          <EstimateRow
            left={`About ${clipMegabytes(rec.bitrateKbps, clipLength(buffer))} MB per clip`}
            right={`Uses up to ${Math.min(memoryBudgetMb(rec, buffer), memoryNeededMb(rec.bitrateKbps, buffer.seconds))} MB memory${
              camMb > 0 ? ` · camera +${camMb} MB` : ""
            }`}
          />
          <SettingsGroup
            title="Memory limit"
            storageKey="video-memory"
            summary={limit.warn ? <span className="text-warn">{limit.text}</span> : limit.text}
          >
            <Row label="Limit">
              <div className="flex w-80 items-center gap-4">
                <div className="min-w-0 flex-1">
                  {/* Let go, then written — the same reason as the length above. */}
                  <Slider
                    label="Memory limit"
                    value={limitMb}
                    min={64}
                    max={4096}
                    step={64}
                    onChange={setMemoryDraft}
                    onRelease={() => setMemoryDraft(null)}
                    onCommit={(memoryMb) => {
                      setMemoryDraft(null);
                      void patchConfig({ buffer: { ...buffer, memoryMb } });
                    }}
                  />
                </div>
                <span className="min-w-16 text-right font-mono text-sm text-ink-muted">
                  {limitMb} MB
                </span>
              </div>
            </Row>
          </SettingsGroup>
        </Card>
      </section>
    </div>
  );
}

/** The one line that says what the settings above cost. */
function EstimateRow({ left, right }: { left: string; right: React.ReactNode }) {
  return (
    <div
      className="flex items-center justify-between gap-6 bg-accent/5 px-5 py-3 text-[13px]
        text-ink-muted [&_b]:font-semibold [&_b]:text-ink"
    >
      <span className="min-w-0">{left}</span>
      <span className="min-w-0 text-right">{right}</span>
    </div>
  );
}

function SourceTile({
  name,
  meta,
  title,
  chip,
  picked,
  warn,
  onPick,
}: {
  name: string;
  meta: string;
  /** Full text for the meta line when it is shortened. */
  title?: string;
  chip?: string;
  picked: boolean;
  warn?: boolean;
  onPick?: () => void;
}) {
  return (
    <Card
      role="button"
      tabIndex={0}
      aria-pressed={picked}
      interactive={onPick !== undefined}
      onClick={onPick}
      onKeyDown={(e) => {
        if (onPick && (e.key === "Enter" || e.key === " ")) {
          e.preventDefault();
          onPick();
        }
      }}
      className={cn(
        "relative min-w-0 px-[18px] py-4 text-left",
        onPick && "cursor-pointer",
        picked && !warn && "border-accent-bright shadow-[0_0_0_1px_var(--color-accent-bright)]",
        warn && "border-warn/60 shadow-[0_0_0_1px_color-mix(in_srgb,var(--color-warn)_60%,transparent)]",
      )}
    >
      {picked && (
        <span
          aria-hidden
          className={cn(
            "absolute top-3.5 right-3.5 grid size-[22px] place-items-center rounded-full text-white",
            warn ? "bg-warn" : "bg-accent",
          )}
        >
          <svg viewBox="0 0 12 12" className="size-3" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <path d="M2.5 6.5 5 9l4.5-5.5" />
          </svg>
        </span>
      )}
      <p className={cn("truncate text-sm font-medium", picked && "pr-8")} title={name}>
        {name}
      </p>
      <p className={cn("mt-1 flex items-center gap-2 text-xs", warn ? "text-warn" : "text-ink-muted")}>
        <span className="min-w-0 truncate" title={title}>
          {meta}
        </span>
        {chip && (
          <span className="shrink-0 rounded-pill bg-elevated px-2 py-px text-[11px] text-ink">
            {chip}
          </span>
        )}
      </p>
    </Card>
  );
}

export function TargetCard({
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
            text-xs font-medium text-white"
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

/** The cam ring's memory — `config::effective_cam_memory_bytes`, in MB. */
function camMemoryMb(config: AppConfig): number {
  const seconds = config.buffer.seconds + config.recording.keyframeSeconds + 12;
  const kbps = config.facecam.resolution === "p1080" ? 6000 : 3000;
  const bytes = Math.max(((kbps * 1000) / 8) * seconds * 1.5, 16 * 1024 * 1024);
  return Math.round(bytes / (1024 * 1024));
}
