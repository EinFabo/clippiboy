import { useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useShallow } from "zustand/react/shallow";
import { useEngine } from "@/store";
import { Card, SectionTitle } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { api, inTauri } from "@/lib/ipc";
import { formatSize } from "@/lib/format";
import type { StorageUsage } from "@/lib/types";

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
 * What ClippiBoy keeps outside the clip folder.
 *
 * These three grow out of sight: an untouched recording per trimmed clip, the
 * individual tracks per clip with more than one source, a thumbnail each. The
 * originals in particular are the reason a trimmed clip can occupy more than an
 * untrimmed one — the clip menu has "Free up space" for that, this says how much
 * there is to get.
 */
function Storage() {
  const [usage, setUsage] = useState<StorageUsage | null>(null);

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
  }, []);

  if (!usage) return null;
  const rows: Array<[string, number, string]> = [
    ["Untouched recordings", usage.originalsBytes, "One per trimmed clip. \u201cFree up space\u201d in the clip menu releases it."],
    ["Individual audio tracks", usage.tracksBytes, "Kept so the mix can still be changed after the fact."],
    ["Thumbnails", usage.thumbsBytes, "One per clip."],
  ];
  const total = usage.originalsBytes + usage.tracksBytes + usage.thumbsBytes;

  return (
    <Card className="mt-3 divide-y divide-line">
      {rows.map(([label, bytes, hint]) => (
        <div key={label} className="flex items-center justify-between gap-6 p-4">
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
        <p className="text-sm font-medium">Beside the clips, all told</p>
        <span className="shrink-0 font-mono text-sm">{formatSize(total)}</span>
      </div>
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
              Reset
            </Button>
          )}
          <Button size="sm" variant="secondary" disabled={!inTauri || busy} onClick={pick}>
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
