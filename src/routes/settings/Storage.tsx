import { useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useShallow } from "zustand/react/shallow";
import { useEngine } from "@/store";
import { Card, SectionTitle } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { api, inTauri } from "@/lib/ipc";
import { formatSize } from "@/lib/format";
import { IconCheck, IconClose } from "@/components/icons";
import { Segmented } from "@/components/ui/Controls";
import { useCountUp } from "@/lib/useCountUp";
import { bakeCardShown } from "@/lib/settingsData";
import { useStorageInfo } from "@/lib/storageInfo";
import type { StorageUsage, TrimOriginals } from "@/lib/types";
import { Row, SettingsGroup } from "./shared";

export function StorageTab() {
  return (
    <div className="space-y-8">
      <section>
        <SectionTitle title="Storage location" />
        <ClipDir />
        <Storage />
      </section>
      <AutoDelete />
    </div>
  );
}

/** "Delete clips older than": in days, `0` is off (`cleanup.rs`). */
const AUTO_DELETE_DAYS = [0, 14, 30, 60, 90] as const;

/**
 * Clean-up by age: clips, screenshots and recordings past the age go to
 * Recently deleted once an hour — favorites never.
 */
function AutoDelete() {
  const { days, patchConfig } = useEngine(
    useShallow((s) => ({ days: s.config.deleteOlderThanDays ?? 0, patchConfig: s.patchConfig })),
  );
  return (
    <section>
      <SectionTitle title="Clean up" />
      <Card className="divide-y divide-line">
        <Row label="Delete clips older than">
          <Segmented<string>
            value={AUTO_DELETE_DAYS.some((d) => d === days) ? String(days) : null}
            options={AUTO_DELETE_DAYS.map((d) => ({
              key: String(d),
              label: d === 0 ? "Off" : `${d} days`,
            }))}
            onChange={(key) => patchConfig({ deleteOlderThanDays: Number(key) })}
          />
        </Row>
      </Card>
    </section>
  );
}

/**
 * What ClippiBoy occupies, all of it.
 *
 * The clips first, then what grows out of sight beside them: an untouched
 * recording per trimmed clip, the individual tracks per clip with more than one
 * source, a thumbnail each, and the web views' cache. The clips were once left
 * out as plainly visible in their folder — and the total then read as a few
 * hundred megabytes beside gigabytes of clips, which looked simply wrong.
 */
function Storage() {
  const { usage, trims, setTrims, prefetch } = useStorageInfo(
    useShallow((s) => ({ usage: s.usage, trims: s.trims, setTrims: s.setTrims, prefetch: s.prefetch })),
  );
  // Settled on what was known when the tab opened, so the card neither pops in
  // nor out while the numbers are read again behind it.
  const [bakeShown, setBakeShown] = useState(() => bakeCardShown(usage?.facecamBytes ?? null));

  // Reads what is missing or stale (Settings already asked as it opened).
  useEffect(() => {
    prefetch();
  }, [prefetch]);

  // After "Clear all trims" or "Bake all": read the folders again.
  const reread = () => prefetch(true);

  return (
    <>
      <UsageCard usage={usage} />
      <ClearTrims found={trims} setFound={setTrims} onCleared={reread} />
      {bakeShown && (
        <BakeCams
          bytes={usage?.facecamBytes ?? null}
          onBaked={() => {
            setBakeShown(false);
            reread();
          }}
        />
      )}
    </>
  );
}

function UsageCard({ usage }: { usage: StorageUsage | null }) {
  // Every row from the first frame; "—" stands for a number not read yet.
  const rows: Array<[string, number | null, string]> = [
    [
      "Clips",
      usage?.clipsBytes ?? null,
      "Your clips, screenshots and recordings, wherever they were saved.",
    ],
    [
      "Recently deleted",
      usage ? (usage.trashBytes ?? 0) : null,
      "Deleted clips, kept for 30 days. Empty them in the gallery.",
    ],
    [
      "Untouched recordings",
      usage?.originalsBytes ?? null,
      "One per trimmed clip, so the trim can be undone. \u201cFree up space\u201d in the clip menu releases one, \u201cClear all trims\u201d below all of them.",
    ],
    [
      "Clean pictures for moving the cam",
      usage?.facecamBytes ?? null,
      "The clean picture and the cam of every clip with a facecam, so the cam can still be moved.",
    ],
    [
      "Screenshot edits",
      usage?.shotEditsBytes ?? null,
      "What an edit can go back to, and what lies under the watermark.",
    ],
    [
      "Individual audio tracks",
      usage?.tracksBytes ?? null,
      "Kept so the mix can still be changed after the fact.",
    ],
    ["Thumbnails", usage?.thumbsBytes ?? null, "One per clip."],
    [
      "App cache",
      usage?.cacheBytes ?? null,
      "What the app's windows keep for themselves. Windows keeps it in check.",
    ],
    [
      "ffmpeg",
      usage?.toolsBytes ?? null,
      "Downloaded once on the first start to write clips.",
    ],
  ];
  const total = usage ? rows.reduce((sum, [, bytes]) => sum + (bytes ?? 0), 0) : null;

  return (
    <Card className="mt-3 divide-y divide-line">
      <SettingsGroup
        title="Everything ClippiBoy takes"
        storageKey="storage.usage"
        summary={<span className="font-mono">{total === null ? "—" : formatSize(total)}</span>}
      >
        {rows.map(([label, bytes, hint]) => (
          <div key={label} className="flex items-center justify-between gap-6 p-4">
            <div>
              <p className="text-sm">{label}</p>
              <p className="mt-0.5 text-xs text-ink-faint">{hint}</p>
            </div>
            <span className="shrink-0 font-mono text-sm text-ink-muted">
              {bytes === null ? "—" : formatSize(bytes)}
            </span>
          </div>
        ))}
      </SettingsGroup>
    </Card>
  );
}

/** How long the freed size takes to count down to nothing. */
const COUNT_MS = 900;

/**
 * "Clear all trims": throws away the untouched recording behind every trimmed
 * clip at once. The single-clip version sits in the clip menu; this is for
 * the day the folder has grown to gigabytes.
 *
 * Asks first, and says what it costs — the trims stay, but none of them can be
 * undone afterwards. Then the size it frees counts down to zero while a bar
 * drains, so the space visibly goes rather than a number just changing.
 */
function ClearTrims({
  found,
  setFound,
  onCleared,
}: {
  found: TrimOriginals | null;
  setFound: (found: TrimOriginals) => void;
  onCleared: () => void;
}) {
  // Keyed on whether the size is known, so the card starts on that size
  // rather than counting up to it from nothing when the answer arrives.
  return (
    <ClearTrimsCard key={found ? "known" : "unknown"} found={found} setFound={setFound} onCleared={onCleared} />
  );
}

function ClearTrimsCard({
  found,
  setFound,
  onCleared,
}: {
  found: TrimOriginals | null;
  setFound: (found: TrimOriginals) => void;
  onCleared: () => void;
}) {
  const discardAll = useEngine((s) => s.discardAllOriginals);
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [freed, setFreed] = useState<TrimOriginals | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Counts down to what is really left — a clip open in a player keeps its
  // original, and the number stops there instead of reaching zero and jumping
  // back. Threshold 0: a few bytes left are still a change worth showing.
  const shownBytes = useCountUp(found?.bytes ?? 0, {
    duration: COUNT_MS,
    threshold: 0,
  });

  const clear = async () => {
    if (!found) return;
    setBusy(true);
    setError(null);
    try {
      const result = await discardAll();
      setAsking(false);
      setFreed(result);
      // What could not go (a clip open in a player) stays counted.
      setFound({
        count: Math.max(0, found.count - result.count),
        bytes: Math.max(0, found.bytes - result.bytes),
      });
      onCleared();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  // Unknown yet: the same card with a dash and a button that waits.
  const nothing = found ? found.count === 0 : true;
  const before = freed ? freed.bytes + (found?.bytes ?? 0) : (found?.bytes ?? 0);
  const share = before > 0 ? shownBytes / before : 0;

  return (
    <Card className="mt-3 p-5">
      <div className="flex items-center justify-between gap-6">
        <div className="min-w-0">
          <p className="text-sm font-medium">Clear all trims</p>
          <p className="mt-0.5 text-xs text-ink-faint">
            {!found
              ? "—"
              : nothing
              ? "No trimmed clip keeps an untouched recording right now."
              : `${found.count} trimmed ${found.count === 1 ? "clip keeps its" : "clips keep their"} untouched recording.`}
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-3">
          <span className="font-mono text-sm tabular-nums text-ink-muted">
            {found ? formatSize(shownBytes) : "—"}
          </span>
          {!asking && (
            <Button
              size="sm"
              variant="danger"
              disabled={nothing || busy}
              onClick={() => setAsking(true)}
            >
              Clear all trims
            </Button>
          )}
        </div>
      </div>

      {freed && freed.count > 0 && shownBytes > 0 && (
        <div className="mt-4 h-1 overflow-hidden rounded-pill bg-elevated">
          <div
            className="h-full rounded-pill bg-accent-bright"
            style={{ width: `${Math.max(0, Math.min(1, share)) * 100}%` }}
          />
        </div>
      )}

      {asking && (
        <div className="cb-freed-in mt-4 rounded-inner border border-warn/40 bg-warn/10 p-4">
          <p className="text-sm font-medium text-warn">This cannot be undone</p>
          <p className="mt-1 text-xs text-ink-muted">
            The untouched recording of every trimmed clip is deleted. The
            trimmed clips stay exactly as they are, but none of their trims can
            be undone or widened again afterwards.
          </p>
          <div className="mt-3 flex justify-end gap-2">
            <Button
              size="sm"
              variant="ghost"
              disabled={busy}
              onClick={() => setAsking(false)}
            >
              Cancel
            </Button>
            <Button
              size="sm"
              variant="danger"
              disabled={busy}
              onClick={() => void clear()}
            >
              {busy ? "Clearing…" : `Free up ${formatSize(found?.bytes ?? 0)}`}
            </Button>
          </div>
        </div>
      )}

      {freed && !asking && (
        <p
          key={freed.bytes}
          className={`cb-freed-in mt-3 flex items-center gap-2 text-xs ${freed.count === 0 ? "text-warn" : "text-ok"}`}
        >
          {freed.count === 0 ? (
            <IconClose className="h-4 w-4" />
          ) : (
            <IconCheck className="h-4 w-4" />
          )}
          {freed.count === 0
            ? "Nothing could be cleared — is a clip open somewhere?"
            : `Freed ${formatSize(freed.bytes)} from ${freed.count} ${freed.count === 1 ? "clip" : "clips"}`}
          {found && found.count > 0 &&
            freed.count > 0 &&
            ` · ${found.count} still in use`}
        </p>
      )}
      {error && <p className="mt-3 text-xs text-live">{error}</p>}
    </Card>
  );
}

/** Folder for new clips — pick, reset, open. */
function ClipDir() {
  const { config, setClipDir } = useEngine(
    useShallow((s) => ({ config: s.config, setClipDir: s.setClipDir })),
  );
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // Only for the "Reset" button: without the comparison the UI would not know
  // whether there is anything to reset at all. Read as Settings opens and
  // remembered, so the button is there (or not) from the first frame.
  const fallback = useStorageInfo((s) => s.defaultDir);

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
        title: "Folder for new clips",
      });
    } catch (err) {
      setError(String(err));
      return;
    }
    // Cancelling the dialog yields null.
    if (typeof picked !== "string") return;
    await apply(picked);
  };

  const canReset = fallback !== null && fallback !== config.clipDir;

  return (
    <>
      <Card className="flex items-center justify-between gap-6 p-5">
        <p
          className="truncate font-mono text-sm text-ink-muted"
          title={config.clipDir}
        >
          {config.clipDir}
        </p>
        <div className="flex shrink-0 gap-2">
          {/* Until the default is known its place is kept, unseen. */}
          {(canReset || (fallback === null && inTauri)) && (
            <Button
              size="sm"
              variant="ghost"
              className={canReset ? undefined : "invisible"}
              disabled={busy || !canReset}
              onClick={() => canReset && apply(fallback)}
            >
              Reset
            </Button>
          )}
          <Button
            size="sm"
            variant="secondary"
            disabled={!inTauri || busy}
            onClick={pick}
          >
            Change
          </Button>
        </div>
      </Card>
      <p className="mt-3 text-xs text-ink-faint">
        Applies to new clips. Ones already saved stay where they are and remain
        playable.
      </p>
      {error && <p className="mt-2 text-xs text-live">{error}</p>}
    </>
  );
}

/**
 * "Bake all": every facecam clip keeps its clean picture and cam so the cam
 * can still be moved. This bakes the cam in where each file shows it and frees
 * that space; the cams can no longer be moved afterwards.
 */
function BakeCams({ bytes, onBaked }: { bytes: number | null; onBaked: () => void }) {
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const bake = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.bakeAllCams();
      setAsking(false);
      onBaked();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card className="mt-3 p-5">
      <div className="flex items-center justify-between gap-6">
        <div className="min-w-0">
          <p className="text-sm font-medium">Bake all cams</p>
          <p className="mt-0.5 text-xs text-ink-faint">
            Frees the clean pictures. The cams stay where they are in each clip.
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-3">
          <span className="font-mono text-sm tabular-nums text-ink-muted">{bytes === null ? "—" : formatSize(bytes)}</span>
          {!asking && (
            <Button size="sm" variant="danger" disabled={busy || !bytes} onClick={() => setAsking(true)}>
              Bake all
            </Button>
          )}
        </div>
      </div>
      {asking && (
        <div className="cb-freed-in mt-4 rounded-inner border border-warn/40 bg-warn/10 p-4">
          <p className="text-sm font-medium text-warn">Bake the cam into every clip?</p>
          <p className="mt-1 text-xs text-ink-muted">The cams can no longer be moved.</p>
          <div className="mt-3 flex justify-end gap-2">
            <Button size="sm" variant="ghost" disabled={busy} onClick={() => setAsking(false)}>
              Cancel
            </Button>
            <Button size="sm" variant="danger" disabled={busy} onClick={() => void bake()}>
              Bake all
            </Button>
          </div>
        </div>
      )}
      {error && <p className="mt-3 text-xs text-live">{error}</p>}
    </Card>
  );
}
