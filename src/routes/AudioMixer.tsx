import { useMemo, useRef, useState } from "react";
import { useEngine } from "@/store";
import { Card, SectionTitle } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { Meter, Slider, Toggle } from "@/components/ui/Controls";
import {
  IconApp,
  IconGamepad,
  IconGrip,
  IconLayers,
  IconMic,
  IconPlus,
  IconSpeaker,
  IconTrash,
} from "@/components/icons";
import { cn } from "@/lib/cn";
import { useFlip } from "@/lib/useFlip";
import type { AudioSource, SourceKind } from "@/lib/types";

/**
 * Does the source get a track of its own by default?
 *
 * The game, the microphone and individual applications yes: those are exactly
 * what you want to turn down or drop entirely in the clip later, and that only
 * works if they were not already folded into the main mix while recording. A
 * output device is the background everything else is lifted out of — that stays
 * the main mix.
 */
function wantsOwnTrack(kind: SourceKind): boolean {
  return kind.type !== "outputDevice";
}

/**
 * Is anything recorded application by application?
 *
 * Then an output device records only what those leave over, otherwise the same
 * sound would be in the clip twice. Nothing to switch: it follows from the
 * sources, and the same rule runs in the core (`audio::records_single_applications`).
 */
function leftoversMode(sources: AudioSource[]): boolean {
  return sources.some(
    (s) =>
      s.enabled &&
      (s.kind.type === "game" ||
        (s.kind.type === "process" && s.kind.mode === "include")),
  );
}

function sourceIcon(kind: SourceKind, leftovers: boolean) {
  if (kind.type === "game") return <IconGamepad className="h-4 w-4" />;
  if (kind.type === "inputDevice") return <IconMic className="h-4 w-4" />;
  if (kind.type === "outputDevice")
    return leftovers ? (
      <IconLayers className="h-4 w-4" />
    ) : (
      <IconSpeaker className="h-4 w-4" />
    );
  return <IconApp className="h-4 w-4" />;
}

function sourceHint(
  kind: SourceKind,
  deviceName: (id: string) => string,
  game: string | null,
  /** The applications this source is actually tapping right now. */
  tapped: string[],
  leftovers: boolean,
) {
  switch (kind.type) {
    case "game":
      return game ? `Game · ${game}` : "Game · none detected right now";
    case "inputDevice":
      return `Input · ${deviceName(kind.deviceId)}`;
    case "outputDevice":
      if (!leftovers) return `Output (loopback) · ${deviceName(kind.deviceId)}`;
      // Which applications it holds is the only thing worth reading here: the
      // device alone no longer tells you what is in the track.
      return tapped.length > 0
        ? `${deviceName(kind.deviceId)} · ${tapped.join(", ")}`
        : `${deviceName(kind.deviceId)} · nothing left to record right now`;
    case "process":
      return kind.mode === "include"
        ? `Application · PID ${kind.pid}`
        : `Everything except PID ${kind.pid}`;
  }
}

/** A source on its way to a new place in the list. */
interface Drag {
  /** The source being carried. */
  id: string;
  /** The gap it is over right now — above or below that row. */
  over: { id: string; before: boolean } | null;
}

export function AudioMixer() {
  const {
    config,
    devices,
    processes,
    levels,
    sourceErrors,
    sourceWarnings,
    detectedGame,
    taps,
    upsertSource,
    removeSource,
    patchConfig,
  } = useEngine();
  const [adding, setAdding] = useState(false);
  /** The source being carried, and the gap it is hovering over. */
  const [drag, setDrag] = useState<Drag | null>(null);
  const list = useRef<HTMLDivElement>(null);

  const deviceName = useMemo(
    () => (id: string) => devices.find((d) => d.id === id)?.name ?? id,
    [devices],
  );
  // Anti-cheat games cannot be named — `list_processes` needs to read their
  // memory and is refused. The bare PID is still better than nothing.
  const processName = useMemo(
    () => (pid: number) =>
      processes.find((p) => p.pid === pid)?.name ?? `PID ${pid}`,
    [processes],
  );

  const anySolo = config.sources.some((s) => s.solo);
  const leftovers = leftoversMode(config.sources);
  const order = config.sources.map((s) => s.id).join(",");
  // A row that has just been dropped somewhere else travels there instead of
  // appearing there — otherwise nothing but the numbers would tell you the drop
  // landed at all.
  useFlip(list, order);

  /**
   * Put the carried source down in the gap it is hovering over.
   *
   * The order is not cosmetic: it is the order of the tracks in the clip, and
   * several ⊘ sources are served from the top down — so it decides which of them
   * gets an application that plays on both.
   */
  const drop = ({ id, over }: Drag) => {
    setDrag(null);
    if (!over) return;
    const rest = config.sources.filter((s) => s.id !== id);
    const source = config.sources.find((s) => s.id === id);
    const at = rest.findIndex((s) => s.id === over.id);
    if (!source || at < 0) return;
    rest.splice(over.before ? at : at + 1, 0, source);
    // Dropping a row back where it came from is not a change and must not cost
    // a config write — the core restarts the audio engine for one.
    if (rest.every((s, index) => s.id === config.sources[index].id)) return;
    patchConfig({ sources: rest });
  };

  /** Which half of the row the pointer is on — that is the gap it means. */
  const hover = (event: React.DragEvent, id: string) => {
    if (!drag) return;
    event.preventDefault();
    event.dataTransfer.dropEffect = "move";
    const box = event.currentTarget.getBoundingClientRect();
    const before = event.clientY < box.top + box.height / 2;
    if (drag.over?.id === id && drag.over.before === before) return;
    setDrag({ ...drag, over: { id, before } });
  };

  /** Move by keyboard — a drag handle nobody can reach with Tab is half a
      control. `delta` is one row up or down. */
  const nudge = (id: string, delta: number) => {
    const from = config.sources.findIndex((s) => s.id === id);
    const to = from + delta;
    if (from < 0 || to < 0 || to >= config.sources.length) return;
    const sources = [...config.sources];
    [sources[from], sources[to]] = [sources[to], sources[from]];
    patchConfig({ sources });
  };

  return (
    <div className="space-y-8">
      <header className="pt-10">
        <h1 className="display text-4xl">Audio mixer</h1>
        <p className="mt-3 max-w-lg text-[15px] leading-relaxed text-white/70">
          Any number of sources at once, each into the main mix or onto a track
          of its own in the clip.
        </p>
      </header>

      <section>
        <SectionTitle
          title="Sources"
          action={
            <Button
              size="sm"
              variant="primary"
              icon={<IconPlus className="h-4 w-4" />}
              onClick={() => setAdding((v) => !v)}
            >
              Add source
            </Button>
          }
        />

        <MixedInHint sources={config.sources} onFix={upsertSource} />

        {adding && (
          <AddSourcePanel
            hasGame={config.sources.some((s) => s.kind.type === "game")}
            onClose={() => setAdding(false)}
            onAdd={(s) => {
              upsertSource(s);
              setAdding(false);
            }}
          />
        )}

        <div className="space-y-3" ref={list}>
          {config.sources.map((source) => {
            const dimmed = anySolo && !source.solo;
            const level = levels[source.id] ?? 0;
            const tapped = (taps[source.id] ?? []).map(processName);
            const carried = drag?.id === source.id;
            const gap = drag?.over?.id === source.id ? drag.over.before : null;
            return (
              <Card
                key={source.id}
                data-flip={source.id}
                onDragOver={(event) => hover(event, source.id)}
                onDrop={(event) => {
                  event.preventDefault();
                  if (drag) drop(drag);
                }}
                className={cn(
                  "relative p-4 transition-opacity duration-200",
                  // Only one opacity, never two: `cn` just joins, so a second
                  // one would not override the first — the stylesheet's own
                  // order would decide.
                  carried
                    ? // The row being carried steps back; the drag image under
                      // the pointer is the one you are watching.
                      "opacity-30"
                    : (!source.enabled || dimmed) && "opacity-45",
                )}
              >
                {gap !== null && (
                  <span
                    className={cn(
                      "pointer-events-none absolute inset-x-3 h-0.5 rounded-full bg-accent-bright",
                      gap ? "-top-2" : "-bottom-2",
                    )}
                  />
                )}
                <div className="flex items-center gap-4">
                  {config.sources.length > 1 && (
                    <span
                      role="button"
                      tabIndex={0}
                      aria-label={`Move ${source.label}`}
                      title="Drag to reorder — the order of the tracks in the clip"
                      draggable
                      onDragStart={(event) => {
                        // The handle is what is dragged, but the row is what
                        // should hang under the pointer.
                        const row = event.currentTarget.closest<HTMLElement>(
                          "[data-flip]",
                        );
                        if (row) {
                          event.dataTransfer.setDragImage(
                            row,
                            32,
                            row.offsetHeight / 2,
                          );
                        }
                        event.dataTransfer.effectAllowed = "move";
                        // Firefox starts no drag at all without a payload.
                        event.dataTransfer.setData("text/plain", source.id);
                        setDrag({ id: source.id, over: null });
                      }}
                      onDragEnd={() => setDrag(null)}
                      onKeyDown={(event) => {
                        const delta =
                          event.key === "ArrowUp"
                            ? -1
                            : event.key === "ArrowDown"
                              ? 1
                              : 0;
                        if (delta === 0) return;
                        event.preventDefault();
                        nudge(source.id, delta);
                      }}
                      className="-mr-1 -ml-1 shrink-0 cursor-grab text-ink-faint transition-colors
                        hover:text-ink focus-visible:text-ink active:cursor-grabbing"
                    >
                      <IconGrip className="h-5 w-5" />
                    </span>
                  )}
                  <span className="grid h-9 w-9 shrink-0 place-items-center rounded-pill bg-elevated text-ink-muted">
                    {sourceIcon(source.kind, leftovers)}
                  </span>

                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <input
                        value={source.label}
                        onChange={(e) =>
                          upsertSource({ ...source, label: e.target.value })
                        }
                        className="w-44 truncate rounded-inner bg-transparent text-sm font-medium
                          outline-none hover:bg-elevated focus:bg-elevated px-1.5 py-0.5"
                      />
                      {source.separateTrack && (
                        <span className="rounded-pill bg-accent/15 px-2 py-0.5 text-[11px] font-medium text-accent-bright">
                          own track
                        </span>
                      )}
                      {/* It is the game source that switches every output
                          device over to its leftovers, so the warning belongs
                          on it rather than on some setting nobody sees. */}
                      {source.kind.type === "game" && (
                        <span
                          title="Recording per application is new. If something ends up missing or doubled, this is where to look first."
                          className="rounded-pill bg-warn/15 px-2 py-0.5 text-[11px] font-medium text-warn"
                        >
                          experimental
                        </span>
                      )}
                    </div>
                    <p
                      className={cn(
                        "mt-0.5 truncate px-1.5 text-xs",
                        sourceErrors[source.id]
                          ? "text-live"
                          : sourceWarnings[source.id]
                            ? "text-warn"
                            : "text-ink-faint",
                      )}
                    >
                      {sourceErrors[source.id] ??
                        sourceWarnings[source.id] ??
                        sourceHint(
                          source.kind,
                          deviceName,
                          detectedGame,
                          tapped,
                          leftovers,
                        )}
                    </p>
                    <div className="mt-2.5 px-1.5">
                      <Meter level={source.muted ? 0 : level} />
                    </div>
                  </div>

                  <div className="flex w-52 shrink-0 items-center gap-3">
                    <Slider
                      label={`Volume ${source.label}`}
                      value={source.gainDb}
                      min={-30}
                      max={12}
                      step={0.5}
                      onChange={(gainDb) => upsertSource({ ...source, gainDb })}
                    />
                    <span className="w-14 shrink-0 text-right font-mono text-xs text-ink-muted">
                      {source.gainDb > 0 ? "+" : ""}
                      {source.gainDb.toFixed(1)} dB
                    </span>
                  </div>

                  <div className="flex shrink-0 items-center gap-1.5">
                    <MiniToggle
                      active={source.muted}
                      activeClass="bg-live/20 text-live"
                      onClick={() =>
                        upsertSource({ ...source, muted: !source.muted })
                      }
                    >
                      M
                    </MiniToggle>
                    <MiniToggle
                      active={source.solo}
                      activeClass="bg-accent/25 text-accent-bright"
                      onClick={() =>
                        upsertSource({ ...source, solo: !source.solo })
                      }
                    >
                      S
                    </MiniToggle>
                    <MiniToggle
                      active={source.separateTrack}
                      activeClass="bg-accent/25 text-accent-bright"
                      title="Own audio track in the MP4"
                      onClick={() =>
                        upsertSource({
                          ...source,
                          separateTrack: !source.separateTrack,
                        })
                      }
                    >
                      ⧉
                    </MiniToggle>
                    <span className="mx-1">
                      <Toggle
                        label={`${source.label} enabled`}
                        checked={source.enabled}
                        onChange={(enabled) =>
                          upsertSource({ ...source, enabled })
                        }
                      />
                    </span>
                    <button
                      aria-label="Remove source"
                      onClick={() => removeSource(source.id)}
                      className="grid h-8 w-8 place-items-center rounded-pill text-ink-faint
                        transition-colors hover:bg-live/15 hover:text-live"
                    >
                      <IconTrash className="h-4 w-4" />
                    </button>
                  </div>
                </div>
              </Card>
            );
          })}
        </div>

        {config.sources.length === 0 && (
          <Card className="grid h-32 place-items-center text-sm text-ink-muted">
            No audio source configured yet.
          </Card>
        )}
      </section>

      <p className="pb-4 text-xs text-ink-faint">
        Game and application sources need process loopback (Windows 10 build
        20348+).
      </p>

      <AvailableSources processes={processes} />
    </div>
  );
}

function MiniToggle({
  active,
  activeClass,
  onClick,
  children,
  title,
}: {
  active: boolean;
  activeClass: string;
  onClick: () => void;
  children: React.ReactNode;
  title?: string;
}) {
  return (
    <button
      title={title}
      onClick={onClick}
      className={cn(
        "h-8 w-8 rounded-pill text-xs font-semibold transition-colors duration-150",
        active ? activeClass : "bg-elevated text-ink-faint hover:text-ink",
      )}
    >
      {children}
    </button>
  );
}

function AvailableSources({
  processes,
}: {
  processes: ReturnType<typeof useEngine.getState>["processes"];
}) {
  if (processes.length === 0) return null;
  return (
    <p className="pb-10 text-xs text-ink-faint">
      Applications detected with audio: {processes.map((p) => p.name).join(" · ")}
    </p>
  );
}

/**
 * Anyone who set ClippiBoy up before this change has microphone and apps in the
 * main mix — they cannot be separated in the clip afterwards. Switching silently
 * would be wrong (it changes what gets recorded), so we ask once.
 */
function MixedInHint({
  sources,
  onFix,
}: {
  sources: AudioSource[];
  onFix: (source: AudioSource) => Promise<void>;
}) {
  const affected = sources.filter((s) => wantsOwnTrack(s.kind) && !s.separateTrack);
  if (affected.length === 0) return null;

  return (
    <Card className="mb-4 flex items-center gap-4 border-accent/40 bg-accent/10 p-4">
      <p className="min-w-0 flex-1 text-[13px] leading-relaxed text-ink-muted">
        <span className="font-medium text-ink">
          {affected.map((s) => s.label).join(", ")}
        </span>{" "}
        {affected.length === 1 ? "runs" : "run"} into the main mix. In finished
        clips {affected.length === 1 ? "it" : "they"} can no longer be muted or
        turned down individually.
      </p>
      <Button
        size="sm"
        className="shrink-0"
        // One after another: each call gets the whole config back, so in
        // parallel the last answer would swallow the other changes.
        onClick={async () => {
          for (const source of affected) {
            await onFix({ ...source, separateTrack: true });
          }
        }}
      >
        Give them their own tracks
      </Button>
    </Card>
  );
}

/** The one source that finds its process on its own. */
function PickOne({
  disabled,
  icon,
  title,
  sub,
  onPick,
}: {
  disabled: boolean;
  icon: React.ReactNode;
  title: string;
  sub: string;
  onPick: () => void;
}) {
  return (
    <button
      disabled={disabled}
      onClick={onPick}
      className={cn(
        "mb-4 flex w-full items-center gap-3 rounded-inner border border-accent/30 bg-accent/10 px-4 py-3 text-left",
        disabled
          ? "cursor-not-allowed opacity-45"
          : "transition-colors hover:bg-accent/20",
      )}
    >
      {icon}
      <span className="min-w-0">
        <span className="flex items-center gap-2 text-sm font-medium">
          {title}
          <span className="rounded-pill bg-warn/15 px-2 py-0.5 text-[11px] font-medium text-warn">
            experimental
          </span>
        </span>
        <span className="block truncate text-[11px] text-ink-faint">{sub}</span>
      </span>
    </button>
  );
}

function AddSourcePanel({
  onAdd,
  onClose,
  hasGame,
}: {
  onAdd: (s: AudioSource) => void;
  onClose: () => void;
  hasGame: boolean;
}) {
  const { devices, processes, refreshSources, detectedGame } = useEngine();
  const outputs = devices.filter((d) => d.kind === "output");
  const inputs = devices.filter((d) => d.kind === "input");

  const make = (label: string, kind: SourceKind): AudioSource => ({
    id: `src-${crypto.randomUUID().slice(0, 8)}`,
    label,
    kind,
    enabled: true,
    gainDb: 0,
    muted: false,
    solo: false,
    separateTrack: wantsOwnTrack(kind),
  });

  return (
    <Card className="mb-4 p-5">
      <div className="mb-4 flex items-center justify-between">
        <h3 className="text-sm font-semibold">Pick a source</h3>
        <div className="flex gap-2">
          <Button size="sm" variant="ghost" onClick={refreshSources}>
            Refresh
          </Button>
          <Button size="sm" variant="ghost" onClick={onClose}>
            Close
          </Button>
        </div>
      </div>

      {/* Above the columns rather than in them: these two are not entries among
          many but the sources that find their processes on their own. */}
      <PickOne
        disabled={hasGame}
        icon={<IconGamepad className="h-4 w-4 shrink-0 text-accent-bright" />}
        title="The detected game"
        sub={
          hasGame
            ? "already added"
            : detectedGame
              ? `follows the game automatically · right now ${detectedGame}`
              : "follows the game automatically · none detected right now"
        }
        onPick={() => onAdd(make("Game", { type: "game" }))}
      />

      <div className="grid grid-cols-3 gap-6">
        <SourceColumn
          title="Applications"
          icon={<IconApp className="h-4 w-4" />}
          entries={processes.map((p) => ({
            key: String(p.pid),
            label: p.name,
            sub: p.exe,
            onPick: () =>
              onAdd(make(p.name, { type: "process", pid: p.pid, mode: "include" })),
          }))}
        />
        <SourceColumn
          title="Outputs (loopback)"
          icon={<IconSpeaker className="h-4 w-4" />}
          entries={outputs.map((d) => ({
            key: d.id,
            label: d.name,
            sub: d.isDefault ? "Default device" : "",
            onPick: () =>
              onAdd(make(d.name, { type: "outputDevice", deviceId: d.id })),
          }))}
        />
        <SourceColumn
          title="Inputs"
          icon={<IconMic className="h-4 w-4" />}
          entries={inputs.map((d) => ({
            key: d.id,
            label: d.name,
            sub: d.isDefault ? "Default device" : "",
            onPick: () =>
              onAdd(make(d.name, { type: "inputDevice", deviceId: d.id })),
          }))}
        />
      </div>
    </Card>
  );
}

function SourceColumn({
  title,
  icon,
  entries,
}: {
  title: string;
  icon: React.ReactNode;
  entries: Array<{
    key: string;
    label: string;
    sub: string;
    onPick: () => void;
  }>;
}) {
  return (
    <div>
      <div className="mb-2 flex items-center gap-2 text-xs font-medium text-ink-muted">
        {icon}
        {title}
      </div>
      <div className="space-y-1">
        {entries.map((e) => (
          <button
            key={e.key}
            onClick={e.onPick}
            className="w-full rounded-inner px-3 py-2 text-left transition-colors hover:bg-elevated"
          >
            <p className="truncate text-sm">{e.label}</p>
            {e.sub && (
              <p className="truncate text-[11px] text-ink-faint">{e.sub}</p>
            )}
          </button>
        ))}
        {entries.length === 0 && (
          <p className="px-3 py-2 text-xs text-ink-faint">nothing found</p>
        )}
      </div>
    </div>
  );
}
