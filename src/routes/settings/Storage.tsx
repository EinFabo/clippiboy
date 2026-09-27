import { useCallback, useEffect, useRef, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useShallow } from "zustand/react/shallow";
import { useEngine } from "@/store";
import { Card, SectionTitle } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { api, inTauri } from "@/lib/ipc";
import { formatSize } from "@/lib/format";
import { IconCheck } from "@/components/icons";
import type { StorageUsage, TrimOriginals } from "@/lib/types";

export function StorageTab() {
  return (
    <section>
      <SectionTitle title="Storage location" />
      <ClipDir />
      <Storage />
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
  const [usage, setUsage] = useState<StorageUsage | null>(null);
  // Bumped after "Clear all trims", so the rows read the folders again.
  const [reads, setReads] = useState(0);

  useEffect(() => {
    if (!inTauri) return;
    let cancelled = false;
    void api
      .storageUsage()
      .then((value) => {
        if (!cancelled) setUsage(value);
      })
      .catch(() => {
        // Nothing to say: an unreadable folder is a missing number, not an error.
      });
    return () => {
      cancelled = true;
    };
  }, [reads]);

  if (!usage) return null;
  const rows: Array<[string, number, string]> = [
    [
      "Clips",
      usage.clipsBytes,
      "Your clips, screenshots and recordings, wherever they were saved.",
    ],
    [
      "Untouched recordings",
      usage.originalsBytes,
      "One per trimmed clip, so the trim can be undone. \u201cFree up space\u201d in the clip menu releases one, \u201cClear all trims\u201d below all of them.",
    ],
    [
      "Individual audio tracks",
      usage.tracksBytes,
      "Kept so the mix can still be changed after the fact.",
    ],
    ["Thumbnails", usage.thumbsBytes, "One per clip."],
    [
      "App cache",
      usage.cacheBytes,
      "What the app's windows keep for themselves. Windows keeps it in check.",
    ],
  ];
  const total = rows.reduce((sum, [, bytes]) => sum + bytes, 0);

  return (
    <>
      <Card className="mt-3 divide-y divide-line">
        {rows.map(([label, bytes, hint]) => (
          <div
            key={label}
            className="flex items-center justify-between gap-6 p-4"
          >
            <div>
              <p className="text-sm">{label}</p>
              <p className="mt-0.5 text-xs text-ink-faint">{hint}</p>
            </div>
            <span className="shrink-0 font-mono text-sm text-ink-muted">
              {formatSize(bytes)}
            </span>
          </div>
        ))}
        <div className="flex items-center justify-between gap-6 p-4">
          <p className="text-sm font-medium">Everything ClippiBoy takes</p>
          <span className="shrink-0 font-mono text-sm">
            {formatSize(total)}
          </span>
        </div>
      </Card>
      <ClearTrims onCleared={() => setReads((n) => n + 1)} />
    </>
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
function ClearTrims({ onCleared }: { onCleared: () => void }) {
  const discardAll = useEngine((s) => s.discardAllOriginals);
  const [found, setFound] = useState<TrimOriginals | null>(null);
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [freed, setFreed] = useState<TrimOriginals | null>(null);
  const [error, setError] = useState<string | null>(null);
  /** The size on screen while it counts down; `null` when not counting. */
  const [counting, setCounting] = useState<number | null>(null);
  const frame = useRef(0);

  const read = useCallback(() => {
    if (!inTauri) {
      setFound({ count: 7, bytes: 3.4e9 });
      return;
    }
    void api
      .trimOriginals()
      .then(setFound)
      .catch(() => setFound({ count: 0, bytes: 0 }));
  }, []);

  useEffect(() => {
    read();
    return () => cancelAnimationFrame(frame.current);
  }, [read]);

  const countDown = (from: number) => {
    const reduced = window.matchMedia(
      "(prefers-reduced-motion: reduce)",
    ).matches;
    if (reduced || from <= 0) {
      setCounting(null);
      return;
    }
    const start = performance.now();
    const step = (now: number) => {
      const t = Math.min(1, (now - start) / COUNT_MS);
      // Ease out: fast at first, settling onto zero.
      const eased = 1 - Math.pow(1 - t, 3);
      setCounting(from * (1 - eased));
      if (t < 1) frame.current = requestAnimationFrame(step);
      else setCounting(null);
    };
    setCounting(from);
    frame.current = requestAnimationFrame(step);
  };

  const clear = async () => {
    if (!found) return;
    setBusy(true);
    setError(null);
    try {
      const result = inTauri ? await discardAll() : found;
      setAsking(false);
      setFreed(result);
      countDown(found.bytes);
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

  if (!found) return null;
  const nothing = found.count === 0;
  const shownBytes = counting ?? found.bytes;
  const before = freed ? freed.bytes + found.bytes : found.bytes;
  const share = before > 0 ? shownBytes / before : 0;

  return (
    <Card className="mt-3 p-5">
      <div className="flex items-center justify-between gap-6">
        <div className="min-w-0">
          <p className="text-sm font-medium">Clear all trims</p>
          <p className="mt-0.5 text-xs text-ink-faint">
            {nothing
              ? "No trimmed clip keeps an untouched recording right now."
              : `${found.count} trimmed ${found.count === 1 ? "clip keeps its" : "clips keep their"} untouched recording.`}
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-3">
          <span className="font-mono text-sm tabular-nums text-ink-muted">
            {formatSize(shownBytes)}
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

      {(counting !== null || (freed && found.bytes > 0)) && (
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
              {busy ? "Clearing…" : `Free up ${formatSize(found.bytes)}`}
            </Button>
          </div>
        </div>
      )}

      {freed && !asking && (
        <p
          key={freed.bytes}
          className="cb-freed-in mt-3 flex items-center gap-2 text-xs text-ok"
        >
          <IconCheck className="h-4 w-4" />
          {freed.count === 0
            ? "Nothing could be cleared — is a clip open somewhere?"
            : `Freed ${formatSize(freed.bytes)} from ${freed.count} ${freed.count === 1 ? "clip" : "clips"}`}
          {found.count > 0 &&
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
  // whether there is anything to reset at all.
  const [fallback, setFallback] = useState<string | null>(null);

  useEffect(() => {
    if (!inTauri) return;
    api
      .defaultClipDir()
      .then(setFallback)
      .catch(() => {});
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
          {canReset && (
            <Button
              size="sm"
              variant="ghost"
              disabled={busy}
              onClick={() => apply(fallback)}
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
