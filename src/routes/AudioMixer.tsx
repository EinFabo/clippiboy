import {
  type ReactNode,
  type RefObject,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";
import { useShallow } from "zustand/react/shallow";
import { useEngine } from "@/store";
import { Card } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { Segmented, Slider, Toggle } from "@/components/ui/Controls";
import { FLOATING, useMenu } from "@/components/ui/Menu";
import { Meter, PeakText } from "@/components/ui/Meter";
import {
  IconApp,
  IconClips,
  IconGamepad,
  IconGrip,
  IconLayers,
  IconMic,
  IconPencil,
  IconPlus,
  IconSpeaker,
  IconSpeakerOff,
  IconTrash,
} from "@/components/icons";
import { cn } from "@/lib/cn";
import { labelToSave } from "@/lib/draft";
import { useFlip } from "@/lib/useFlip";
import { gainText, sourceStatus } from "@/lib/audioRows";
import { placeMenu } from "@/lib/menuPlace";
import { discordKind, hasDiscordSource, runningDiscord } from "@/lib/discordSource";
import type { AudioProcess, AudioSource, SourceKind } from "@/lib/types";
import { appRows } from "@/lib/appPicker";
import { api, inTauri } from "@/lib/ipc";

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
 * The name field of a source, shown while it is being renamed. It keeps its own
 * draft and hands the name over when the field is left or Enter is pressed —
 * not on every keystroke, which wrote the config (and could eat characters
 * while the core's answer landed). Escape puts the saved name back.
 */
function LabelField({
  label,
  onSave,
  onDone,
}: {
  label: string;
  onSave: (label: string) => void;
  onDone: () => void;
}) {
  const [draft, setDraft] = useState<string | null>(null);
  // Escape blurs the field, and that would otherwise save what was cancelled.
  const cancelled = useRef(false);
  const commit = () => {
    const next = labelToSave(draft, label);
    setDraft(null);
    if (!cancelled.current && next !== null) onSave(next);
    cancelled.current = false;
    onDone();
  };
  return (
    <input
      autoFocus
      onFocus={(e) => e.currentTarget.select()}
      aria-label={`Name of ${label}`}
      value={draft ?? label}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") e.currentTarget.blur();
        else if (e.key === "Escape") {
          cancelled.current = true;
          e.currentTarget.blur();
        }
      }}
      className="w-full truncate rounded-inner bg-elevated px-1.5 py-0.5 text-sm font-medium outline-none"
    />
  );
}

/** A new source as both "Quick setup" and "Pick a source" add it. */
function newSource(label: string, kind: SourceKind): AudioSource {
  return {
    id: `src-${crypto.randomUUID().slice(0, 8)}`,
    label,
    kind,
    enabled: true,
    gainDb: 0,
    muted: false,
    solo: false,
    separateTrack: wantsOwnTrack(kind),
  };
}

/**
 * Discord on a track of its own, so friends' voices can be turned down in the
 * clip. Quick setup and the note after the first clip add the same source.
 */
export function discordSource(process: AudioProcess): AudioSource {
  return newSource("Discord", discordKind(process));
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

/** The volume slider's range, in dB. */
const GAIN_MIN = -30;
const GAIN_MAX = 12;
/** The slider's knob is 14 px wide: its centre runs from 7 px to the width less 7 px. */
const THUMB_PX = 14;

/**
 * A source's volume as one track, the way Windows' own mixer draws it: the part
 * up to the knob in the dark accent (what is set), the live signal bright inside
 * that part — how full it is, on the meter's dB scale, so it never passes the
 * knob — and the slider's own knob above both. The input stays the control, for
 * the pointer and the keyboard alike. The knob gets a warm ring while the level
 * after the volume is about to clip (`.cb-vol[data-hot]` in index.css). Muted
 * or off, the bright part stays away and the track dark.
 */
function VolumeTrack({
  source,
  gainDb,
  min,
  max,
  className,
  children,
}: {
  source: AudioSource;
  gainDb: number;
  min: number;
  max: number;
  className?: string;
  children: ReactNode;
}) {
  const box = useRef<HTMLDivElement>(null);
  const share = (gainDb - min) / (max - min);
  // Up to the knob's centre, which the browser keeps inside the track.
  const setWidth = { width: `calc(${THUMB_PX / 2}px + (100% - ${THUMB_PX}px) * ${share})` };
  return (
    <div
      ref={box}
      className={cn("cb-vol relative flex h-4 min-w-[110px] flex-1 items-center", className)}
    >
      <div className="pointer-events-none absolute inset-x-0 top-1/2 h-1.5 -translate-y-1/2 rounded-pill bg-line">
        <div className="absolute inset-y-0 left-0 rounded-pill bg-accent-deep" style={setWidth} />
        <Meter
          of={{ source: source.id }}
          muted={source.muted || !source.enabled}
          gainDb={source.gainDb}
          hot={box}
          style={setWidth}
        />
      </div>
      <div className="relative flex w-full items-center">{children}</div>
    </div>
  );
}

/**
 * The first row of the list: what the clip's main track carries right now, in
 * the same columns as the sources below it. Left and right as two fine tracks,
 * the peak where the sources show their volume, and nothing to set. The level
 * is the core's estimate from the sources in the main track
 * (`audio::engine::main_mix_level`); sources on a track of their own are only
 * counted in the line below the name.
 */
function ClipMixRow({ sources }: { sources: AudioSource[] }) {
  const own = sources.filter((s) => s.enabled && s.separateTrack).length;
  return (
    <div className="flex items-center gap-3.5 bg-accent/[0.06] px-4 py-3">
      <span className="w-5 shrink-0" />
      <span className="grid h-[34px] w-[34px] shrink-0 place-items-center rounded-pill bg-elevated text-ink-muted">
        <IconClips className="h-4 w-4" />
      </span>
      <div className="w-[190px] min-w-0 shrink-0">
        <p className="truncate px-1.5 py-0.5 text-sm font-medium">In your clip</p>
        <p className="truncate px-1.5 text-xs text-ink-faint">
          {own > 0
            ? `Main track · ${own} own track${own === 1 ? "" : "s"}`
            : "Main track"}
        </p>
      </div>
      <div
        role="img"
        aria-label="Level of the main track, left and right"
        className="flex min-w-[110px] flex-1 flex-col gap-[5px]"
      >
        {(["l", "r"] as const).map((side) => (
          <div key={side} className="relative h-1 rounded-pill bg-line">
            <Meter of={{ mix: side }} />
          </div>
        ))}
      </div>
      <PeakText
        of={{ mix: "peak" }}
        className="w-[60px] shrink-0 text-right font-mono text-xs text-ink-muted"
      />
      {/* No controls here, but the room of the source rows' own (mute 30, track
          84, switch 44, menu 30 and three gaps of 14), so the columns line up. */}
      <span aria-hidden className="w-[230px] shrink-0" />
    </div>
  );
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
    sourceErrors,
    sourceWarnings,
    detectedGame,
    taps,
    upsertSource,
    removeSource,
    patchConfig,
    // `levels` steht mit Absicht nicht hier: die Pegel kommen zwanzigmal die
    // Sekunde, und über diese Zeile hätte der ganze Mischer sie mitgemacht.
    // Jeder Balken holt sich seinen eigenen Wert — siehe `Meter`.
  } = useEngine(
    useShallow((s) => ({
      config: s.config,
      devices: s.devices,
      processes: s.processes,
      sourceErrors: s.sourceErrors,
      sourceWarnings: s.sourceWarnings,
      detectedGame: s.detectedGame,
      taps: s.taps,
      upsertSource: s.upsertSource,
      removeSource: s.removeSource,
      patchConfig: s.patchConfig,
    })),
  );
  const menu = useMenu();
  const [adding, setAdding] = useState(false);
  const addButton = useRef<HTMLSpanElement>(null);
  /** The source whose name is being edited. */
  const [renaming, setRenaming] = useState<string | null>(null);
  /** The gain of the source whose knob is being dragged; written on release. */
  const [gainDraft, setGainDraft] = useState<{ id: string; gainDb: number } | null>(null);
  /** The source being carried, and the gap it is hovering over. */
  const [drag, setDrag] = useState<Drag | null>(null);
  const list = useRef<HTMLDivElement>(null);


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

  /** The "⋯" menu of one row. */
  const openMenu = (event: React.MouseEvent<HTMLButtonElement>, source: AudioSource) => {
    const box = event.currentTarget.getBoundingClientRect();
    menu.open(
      {
        clientX: box.right,
        clientY: box.bottom + 6,
        preventDefault: () => {},
        stopPropagation: () => {},
      },
      [
        {
          kind: "item",
          label: "Rename",
          icon: <IconPencil className="h-4 w-4" />,
          onSelect: () => setRenaming(source.id),
        },
        {
          kind: "item",
          label: "Remove",
          icon: <IconTrash className="h-4 w-4" />,
          danger: true,
          onSelect: () => void removeSource(source.id),
        },
      ],
      { alignRight: true, anchor: box, gap: 6 },
    );
  };

  return (
    <div className="space-y-6 pb-10">
      <header className="flex items-end justify-between pt-10">
        <h1 className="display text-4xl">Audio</h1>
        <span ref={addButton}>
          <Button
            size="sm"
            variant="primary"
            icon={<IconPlus className="h-4 w-4" />}
            onClick={() => setAdding((v) => !v)}
          >
            Add source
          </Button>
        </span>
      </header>

      {adding && (
        <AddSourceList
          anchor={addButton}
          hasGame={config.sources.some((s) => s.kind.type === "game")}
          onClose={() => setAdding(false)}
          onAdd={(s) => {
            void upsertSource(s);
            setAdding(false);
          }}
        />
      )}

      {config.sources.length > 0 && (
        <div ref={list}>
        <Card className="overflow-hidden">
          <ClipMixRow sources={config.sources} />
          {config.sources.map((source) => {
            const status = sourceStatus(source, {
              devices,
              processes,
              detectedGame,
              error: sourceErrors[source.id],
              warning: sourceWarnings[source.id],
              tapped: (taps[source.id] ?? []).map((t) => t.name),
              leftovers,
            });
            const carried = drag?.id === source.id;
            const gap = drag?.over?.id === source.id ? drag.over.before : null;
            const shownGain =
              gainDraft?.id === source.id ? gainDraft.gainDb : source.gainDb;
            return (
              <div
                key={source.id}
                data-flip={source.id}
                onDragOver={(event) => hover(event, source.id)}
                onDrop={(event) => {
                  event.preventDefault();
                  if (drag) drop(drag);
                }}
                className={cn(
                  "relative flex items-center gap-3.5 border-t border-line px-4 py-3 transition-[opacity,background-color] duration-200",
                  status.tone === "warn" && "bg-warn/5",
                  status.tone === "error" && "bg-live/5",
                  // Only one opacity, never two: `cn` just joins, so a second
                  // one would not override the first.
                  carried
                    ? // The row being carried steps back; the drag image under
                      // the pointer is the one you are watching.
                      "opacity-30"
                    : !source.enabled && "opacity-55",
                )}
              >
                {gap !== null && (
                  <span
                    className={cn(
                      "pointer-events-none absolute inset-x-3 z-10 h-0.5 rounded-full bg-accent-bright",
                      gap ? "-top-px" : "-bottom-px",
                    )}
                  />
                )}
                <span className="w-5 shrink-0">
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
                      className="relative block cursor-grab text-ink-faint transition-colors after:absolute after:-inset-1 after:content-['']
                        hover:text-ink focus-visible:text-ink active:cursor-grabbing"
                    >
                      <IconGrip className="h-5 w-5" />
                    </span>
                  )}
                </span>
                <span className="grid h-[34px] w-[34px] shrink-0 place-items-center rounded-pill bg-elevated text-ink-muted">
                  {sourceIcon(source.kind, leftovers)}
                </span>

                <div className="w-[190px] min-w-0 shrink-0">
                  {renaming === source.id ? (
                    <LabelField
                      label={source.label}
                      onSave={(label) => void upsertSource({ ...source, label })}
                      onDone={() => setRenaming(null)}
                    />
                  ) : (
                    <p
                      className="truncate px-1.5 py-0.5 text-sm font-medium"
                      onDoubleClick={() => setRenaming(source.id)}
                    >
                      {source.label}
                    </p>
                  )}
                  <p
                    title={status.text}
                    className={cn(
                      "truncate px-1.5 text-xs",
                      status.tone === "error"
                        ? "text-live"
                        : status.tone === "warn"
                          ? "text-warn"
                          : "text-ink-faint",
                    )}
                  >
                    {status.text}
                  </p>
                </div>

                <VolumeTrack
                  source={source}
                  gainDb={shownGain}
                  min={GAIN_MIN}
                  max={GAIN_MAX}
                  className={cn(status.tone === "warn" && "opacity-50")}
                >
                  <Slider
                    clear
                    label={`Volume ${source.label}`}
                    value={shownGain}
                    min={GAIN_MIN}
                    max={GAIN_MAX}
                    step={0.5}
                    onChange={(gainDb) =>
                      setGainDraft({ id: source.id, gainDb })
                    }
                    onCommit={(gainDb) => {
                      setGainDraft(null);
                      if (gainDb !== source.gainDb)
                        void upsertSource({ ...source, gainDb });
                    }}
                    onRelease={() => setGainDraft(null)}
                  />
                </VolumeTrack>
                <span className="w-[60px] shrink-0 text-right font-mono text-xs text-ink-muted">
                  {gainText(shownGain)}
                </span>

                <button
                  aria-label={`Mute ${source.label}`}
                  aria-pressed={source.muted}
                  title={source.muted ? "Muted" : "Mute"}
                  onClick={() =>
                    void upsertSource({ ...source, muted: !source.muted })
                  }
                  className={cn(
                    "grid h-[30px] w-[30px] shrink-0 place-items-center rounded-pill transition-colors duration-150",
                    source.muted
                      ? "bg-live/20 text-live"
                      : "bg-elevated text-ink-faint hover:text-ink",
                  )}
                >
                  {source.muted ? (
                    <IconSpeakerOff className="h-4 w-4" />
                  ) : (
                    <IconSpeaker className="h-4 w-4" />
                  )}
                </button>
                <button
                  aria-label={`Own track for ${source.label}`}
                  aria-pressed={source.separateTrack}
                  title="Main mix, or an audio track of its own in the clip"
                  onClick={() =>
                    void upsertSource({
                      ...source,
                      separateTrack: !source.separateTrack,
                    })
                  }
                  className={cn(
                    "inline-flex h-[26px] w-[84px] shrink-0 items-center justify-center rounded-pill border text-xs whitespace-nowrap transition-colors duration-150",
                    source.separateTrack
                      ? "border-accent/60 bg-accent/15 text-accent-bright"
                      : "border-line-strong text-ink-faint hover:text-ink",
                  )}
                >
                  {source.separateTrack ? "Own track" : "Main mix"}
                </button>
                <Toggle
                  label={`${source.label} enabled`}
                  checked={source.enabled}
                  onChange={(enabled) => void upsertSource({ ...source, enabled })}
                />
                <button
                  aria-label={`More for ${source.label}`}
                  onClick={(event) => openMenu(event, source)}
                  className="grid h-[30px] w-[30px] shrink-0 place-items-center rounded-pill text-lg leading-none text-ink-faint
                    transition-colors hover:bg-elevated hover:text-ink"
                >
                  ⋯
                </button>
              </div>
            );
          })}
        </Card>
        </div>
      )}

      {config.sources.length === 0 && (
        <QuickSetup onAdd={upsertSource} onPick={() => setAdding(true)} />
      )}
    </div>
  );
}

/**
 * Quick audio setup: in place of an empty source list — a fresh install, or
 * every source removed — the two that nearly everyone wants, one click away.
 * What you hear and your microphone, pinned to the devices Windows has as its
 * defaults right now and named after them, exactly as picking them by hand
 * would. Not "follow the default" (an empty id): the core rightly warns about
 * a source whose name and device can drift apart, and a setup that raises a
 * warning the moment it is done is the wrong first impression. Game audio is
 * left out on purpose: the output already carries it.
 */
export function QuickSetup({
  onAdd,
  onPick,
  pickLabel = "Pick sources myself",
}: {
  onAdd: (source: AudioSource) => Promise<void>;
  onPick: () => void;
  pickLabel?: string;
}) {
  const devices = useEngine((s) => s.devices);
  const sources = useEngine((s) => s.config.sources);
  const processes = useEngine((s) => s.processes);
  const refreshSources = useEngine((s) => s.refreshSources);
  // A Discord started since the lists were read still gets its row.
  useEffect(() => {
    void refreshSources();
  }, [refreshSources]);
  const appCapture = useEngine((s) => s.appCapture);
  // Discord has its own track only through per-app capture.
  const discord = appCapture ? runningDiscord(processes) : undefined;
  const discordAdded = hasDiscordSource(sources);
  const output = devices.find((d) => d.kind === "output" && d.isDefault);
  const input = devices.find((d) => d.kind === "input" && d.isDefault);
  // The setup shows this with sources already there too: what is in the list
  // already is marked, not added a second time.
  const has = (type: "outputDevice" | "inputDevice", id: string | undefined) =>
    id !== undefined &&
    sources.some((s) => s.kind.type === type && "deviceId" in s.kind && s.kind.deviceId === id);
  const outputAdded = has("outputDevice", output?.id);
  const inputAdded = has("inputDevice", input?.id);
  const [withOutput, setWithOutput] = useState(true);
  const [withInput, setWithInput] = useState(true);
  const [withDiscord, setWithDiscord] = useState(true);
  const [busy, setBusy] = useState(false);

  const add = async () => {
    setBusy(true);
    try {
      // One after the other: each call writes the whole configuration.
      if (withOutput && output && !outputAdded) {
        await onAdd(newSource(output.name, { type: "outputDevice", deviceId: output.id }));
      }
      if (withInput && input && !inputAdded) {
        await onAdd(newSource(input.name, { type: "inputDevice", deviceId: input.id }));
      }
      if (withDiscord && discord && !discordAdded) {
        await onAdd(discordSource(discord));
      }
    } finally {
      setBusy(false);
    }
  };

  const rows: Array<{
    on: boolean;
    set: (on: boolean) => void;
    added: boolean;
    icon: ReactNode;
    title: string;
    device: string;
  }> = [
    {
      on: withOutput,
      set: setWithOutput,
      added: outputAdded,
      icon: <IconSpeaker className="h-4 w-4" />,
      title: "Desktop audio",
      device: output ? output.name : "",
    },
  ];
  if (!output) rows.pop();
  if (input) {
    rows.push({
      on: withInput,
      set: setWithInput,
      added: inputAdded,
      icon: <IconMic className="h-4 w-4" />,
      title: "Microphone",
      device: input.name,
    });
  }
  if (appCapture && (discord || discordAdded)) {
    rows.push({
      on: withDiscord,
      set: setWithDiscord,
      added: discordAdded,
      icon: <IconApp className="h-4 w-4" />,
      title: "Discord",
      device: "Friends' voices on their own track",
    });
  }
  const nothing =
    !(withOutput && output && !outputAdded) &&
    !(withInput && input && !inputAdded) &&
    !(withDiscord && discord && !discordAdded);
  const allAdded = rows.length > 0 && rows.every((row) => row.added);

  return (
    <Card className="p-5">
      <h3 className="text-sm font-semibold">Quick setup</h3>
      <p className="mt-1 text-xs text-ink-muted">
        {sources.length === 0
          ? "No audio source yet, so your clips would be silent. "
          : ""}
        Add what you hear and your microphone — the devices Windows uses right now. You can
        change them later.
      </p>
      <div className="mt-4 grid gap-2">
        {rows.map((row) => (
          <div
            key={row.title}
            className="flex items-center justify-between gap-4 rounded-inner border border-line bg-elevated px-4 py-3"
          >
            <div className="flex min-w-0 items-center gap-3">
              <span className="text-ink-muted">{row.icon}</span>
              <div className="min-w-0">
                <p className="text-sm">{row.title}</p>
                <p className="truncate text-xs text-ink-faint">{row.device}</p>
              </div>
            </div>
            {row.added ? (
              <span className="shrink-0 rounded-pill bg-ok/15 px-2.5 py-0.5 text-xs font-medium text-ok">
                Added
              </span>
            ) : (
              <Toggle checked={row.on} onChange={row.set} />
            )}
          </div>
        ))}
      </div>
      <div className="mt-4 flex items-center justify-end gap-2">
        <Button size="sm" variant="ghost" onClick={onPick}>
          {pickLabel}
        </Button>
        <Button size="sm" variant="primary" disabled={busy || nothing} onClick={() => void add()}>
          {busy ? "Adding…" : allAdded ? "All added" : "Add"}
        </Button>
      </div>
    </Card>
  );
}


/** Distance to the window edge, and the space between button and list. */
const EDGE = 8;
const GAP = 6;

function Experimental() {
  return (
    <span
      title="Recording per application is new. If something ends up missing or doubled, this is where to look first."
      className="rounded-pill bg-warn/15 px-2 py-0.5 text-[11px] font-medium text-warn"
    >
      experimental
    </span>
  );
}

/**
 * The list behind "Add source": Apps (the detected game on top, then the
 * running applications with audio) and Devices (outputs, then inputs). A glass
 * popover under its button, placed like every menu; Escape, a click outside or
 * a resize closes it.
 */
function AddSourceList({
  anchor,
  hasGame,
  onAdd,
  onClose,
}: {
  anchor: RefObject<HTMLElement | null>;
  hasGame: boolean;
  onAdd: (s: AudioSource) => void;
  onClose: () => void;
}) {
  const { devices, processes, refreshSources, detectedGame, appCapture, sources } = useEngine(
    useShallow((s) => ({
      sources: s.config.sources,
      appCapture: s.appCapture,
      devices: s.devices,
      processes: s.processes,
      refreshSources: s.refreshSources,
      detectedGame: s.detectedGame,
    })),
  );
  const [tab, setTab] = useState<"apps" | "devices">("apps");
  const box = useRef<HTMLDivElement>(null);
  const [place, setPlace] = useState<{ left: number; top: number; maxHeight: number } | null>(
    null,
  );
  const outputs = devices.filter((d) => d.kind === "output");
  const inputs = devices.filter((d) => d.kind === "input");

  // The list is measured at its natural size, so a tab change re-places it.
  useLayoutEffect(() => {
    const el = box.current;
    const button = anchor.current;
    if (!el || !button) return;
    const kept = el.style.maxHeight;
    el.style.maxHeight = "none";
    const p = placeMenu({
      anchor: button.getBoundingClientRect(),
      size: { width: el.offsetWidth, height: el.offsetHeight },
      viewport: { width: window.innerWidth, height: window.innerHeight },
      margin: EDGE,
      gap: GAP,
      alignRight: true,
      slide: false,
    });
    el.style.maxHeight = kept;
    setPlace({ left: p.x, top: p.y, maxHeight: Math.min(el.offsetHeight, p.maxHeight) });
  }, [anchor, tab, processes.length, devices.length]);

  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    const onDown = (event: PointerEvent) => {
      const target = event.target as Node;
      if (box.current?.contains(target) || anchor.current?.contains(target)) return;
      close.current();
    };
    const onScroll = (event: Event) => {
      if (event.target instanceof Node && box.current?.contains(event.target)) return;
      close.current();
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      anchor.current?.querySelector<HTMLElement>("button")?.focus();
      close.current();
    };
    const onAway = () => close.current();
    window.addEventListener("pointerdown", onDown, true);
    window.addEventListener("scroll", onScroll, true);
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("resize", onAway);
    window.addEventListener("blur", onAway);
    return () => {
      window.removeEventListener("pointerdown", onDown, true);
      window.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("resize", onAway);
      window.removeEventListener("blur", onAway);
    };
  }, [anchor]);

  // A Discord or game started since the lists were read still shows up, and
  // one that starts while the list is open appears without a click.
  useEffect(() => {
    void refreshSources();
    if (!inTauri) return;
    const timer = window.setInterval(() => {
      void api
        .listAudioProcesses()
        .then((next) => {
          if (JSON.stringify(next) !== JSON.stringify(useEngine.getState().processes)) {
            useEngine.setState({ processes: next });
          }
        })
        .catch(() => {});
    }, 2000);
    return () => window.clearInterval(timer);
  }, [refreshSources]);

  const entry = (
    key: string,
    label: string,
    sub: string,
    kind: SourceKind,
    icon: ReactNode,
    added = false,
  ) => (
    <button
      key={key}
      disabled={added}
      onClick={() => onAdd(newSource(label, kind))}
      className={cn(
        "flex w-full items-center gap-2.5 rounded-[10px] px-2.5 py-1.5 text-left transition-colors",
        added ? "cursor-default opacity-60" : "hover:bg-hover focus-visible:bg-hover",
      )}
    >
      <span className="grid h-7 w-7 shrink-0 place-items-center rounded-pill bg-elevated text-ink-muted">
        {icon}
      </span>
      <span className="min-w-0 flex-1">
        <span className="block truncate text-[13px] font-medium">{label}</span>
        {sub && <span className="block truncate text-xs text-ink-faint">{sub}</span>}
      </span>
      {added && <span className="shrink-0 text-xs text-ink-muted">Added</span>}
    </button>
  );
  const heading = (text: string, extra?: ReactNode) => (
    <div className="flex items-center gap-2 px-2.5 pt-2 pb-1 text-xs font-medium text-ink-muted">
      {text}
      {extra}
    </div>
  );
  const empty = <p className="px-2.5 py-2 text-xs text-ink-faint">Nothing found</p>;

  return createPortal(
    <div
      ref={box}
      role="dialog"
      aria-label="Add source"
      style={{
        left: place?.left ?? 0,
        top: place?.top ?? 0,
        maxHeight: place?.maxHeight,
        background: FLOATING,
        backdropFilter: "blur(20px)",
      }}
      className={cn(
        "fixed z-[70] flex w-[380px] flex-col rounded-inner border border-white/10 p-1.5",
        "shadow-[0_8px_32px_rgba(0,0,0,0.4)] origin-top-right",
        "transition-[opacity,transform] duration-100 ease-[var(--ease-out-soft)]",
        place ? "scale-100 opacity-100" : "scale-[0.97] opacity-0",
      )}
    >
      <div className="flex shrink-0 items-center justify-between gap-2 p-0.5 pb-1.5">
        <Segmented<"apps" | "devices">
          value={tab}
          onChange={setTab}
          options={[
            { key: "apps", label: "Apps" },
            { key: "devices", label: "Devices" },
          ]}
        />
        <Button size="sm" variant="ghost" onClick={() => void refreshSources()}>
          Refresh
        </Button>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain">
        {tab === "apps" ? (
          <>
            {!appCapture && (
              <p className="mx-1 my-1 px-2.5 py-2 text-xs text-ink-muted">
                Per-app audio needs Windows 11
              </p>
            )}
            {appCapture && !hasGame && (
              <div className="flex items-center gap-2.5 rounded-[10px] px-2.5 py-1.5">
                <span className="grid h-7 w-7 shrink-0 place-items-center rounded-pill bg-accent/15 text-accent-bright">
                  <IconGamepad className="h-4 w-4" />
                </span>
                <span className="min-w-0 flex-1 truncate text-[13px] font-medium">
                  {detectedGame ? `Detected game: ${detectedGame}` : "Game · none detected"}
                </span>
                <Button
                  size="sm"
                  onClick={() => onAdd(newSource("Game", { type: "game" }))}
                >
                  Add
                </Button>
              </div>
            )}
            {appCapture && heading("Applications", <Experimental />)}
            {!appCapture
              ? null
              : processes.length === 0
              ? empty
              : appRows(processes, sources, detectedGame).map(({ process: p, added, sub }) =>
                  entry(
                    String(p.pid),
                    p.name,
                    sub,
                    // Stored alongside the pid: the pid is dead the moment the
                    // application restarts, the exe name is not.
                    { type: "process", pid: p.pid, exe: p.exe, mode: "include" },
                    <IconApp className="h-4 w-4" />,
                    added,
                  ),
                )}
          </>
        ) : (
          <>
            {heading("Outputs (loopback)")}
            {outputs.length === 0
              ? empty
              : outputs.map((d) =>
                  entry(
                    d.id,
                    d.name,
                    d.isDefault ? "Default device" : "",
                    { type: "outputDevice", deviceId: d.id },
                    <IconSpeaker className="h-4 w-4" />,
                  ),
                )}
            {heading("Inputs")}
            {inputs.length === 0
              ? empty
              : inputs.map((d) =>
                  entry(
                    d.id,
                    d.name,
                    d.isDefault ? "Default device" : "",
                    { type: "inputDevice", deviceId: d.id },
                    <IconMic className="h-4 w-4" />,
                  ),
                )}
          </>
        )}
      </div>
    </div>,
    document.fullscreenElement ?? document.body,
  );
}
