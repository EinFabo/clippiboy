import { useEffect, useState } from "react";
import { create } from "zustand";
import { cn } from "@/lib/cn";
import { megabytes, shareApi, useShare } from "@/lib/share";
import { missedDetail, missedLine, noteMissed, shownMissed } from "@/lib/missedSends";
import { shownName, useStreamer } from "@/lib/streamer";
import type { Transfer } from "@/lib/types";

/** When each send of one's own that did not get there was first seen here. */
const useMissed = create<{ seen: Record<string, number> }>(() => ({ seen: {} }));

function noteNow() {
  const before = useMissed.getState().seen;
  const seen = noteMissed(useShare.getState().transfers, before, Date.now());
  if (seen !== before) useMissed.setState({ seen });
}
// From the start, not only while the list is drawn: the time a no came is
// what decides how long it stays.
useShare.subscribe(noteNow);
noteNow();

/**
 * Clips, die ein Freund schicken will — oben in der Konsole, zum Annehmen im
 * Spiel. Das Banner darüber kann niemand anklicken (es lässt jeden Klick ans
 * Spiel durch); es sagt nur, dass die Antwort hier liegt.
 *
 * Kostet kein eigenes Backend: `share-state` geht an jedes Fenster, und der
 * Store in `lib/share.ts` hört in diesem hier genauso zu wie im Hauptfenster.
 */
export function Offers({ onError }: { onError: (text: string) => void }) {
  const transfers = useShare((s) => s.transfers);
  const seen = useMissed((s) => s.seen);
  const [now, setNow] = useState(() => Date.now());
  const shown = transfers.filter((t) => t.direction === "in" && (t.stage === "asking" || t.stage === "moving"));
  const missed = shownMissed(transfers, seen, now);
  const anyMissed = missed.length > 0;
  // A miss leaves the list after a while, open or not.
  useEffect(() => {
    if (!anyMissed) return;
    const timer = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(timer);
  }, [anyMissed]);
  if (shown.length === 0 && !anyMissed) return null;
  return (
    <div className="pointer-events-auto absolute top-8 left-1/2 flex w-[380px] -translate-x-1/2 flex-col gap-2">
      {shown.map((t) => (
        <Offer key={t.id} transfer={t} onError={onError} />
      ))}
      {missed.map((t) => (
        <Missed key={t.id} transfer={t} onError={onError} />
      ))}
    </div>
  );
}

/** A send of one's own that did not get there — a no, no answer, a failure —
 *  with Retry, as the banner that told of it promised. */
function Missed({ transfer: t, onError }: { transfer: Transfer; onError: (text: string) => void }) {
  const [busy, setBusy] = useState(false);
  const streamer = useStreamer((s) => s.active);
  const clipId = t.clipId;
  const retry = () => {
    if (!clipId) return;
    setBusy(true);
    shareApi
      .send(clipId, t.friendId)
      // The new send has a line of its own; this one has done its job.
      .then(() => shareApi.dismiss(t.id))
      .catch((err) => onError(String(err)))
      .finally(() => setBusy(false));
  };
  const dismiss = () => {
    shareApi.dismiss(t.id).catch((err) => onError(String(err)));
  };

  return (
    <div className="cb-glass flex items-center gap-3 rounded-[18px] px-4 py-3.5">
      <div className="min-w-0 flex-1">
        <p className="truncate text-[13px] font-semibold">{missedLine(t, shownName(t.friendName, streamer))}</p>
        <p className="truncate text-xs text-ink-muted">{missedDetail(t)}</p>
      </div>
      <button
        aria-label="Dismiss"
        onClick={dismiss}
        className="grid size-7 shrink-0 place-items-center rounded-pill text-ink-muted transition-colors hover:bg-white/10 hover:text-ink"
      >
        <svg viewBox="0 0 12 12" className="size-3" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round">
          <path d="M3 3l6 6M9 3l-6 6" />
        </svg>
      </button>
      {clipId && (
        <button
          disabled={busy}
          onClick={retry}
          className={cn(
            "shrink-0 rounded-pill bg-white px-3.5 py-1.5 text-xs font-semibold text-black transition-opacity",
            busy && "opacity-60",
          )}
        >
          Retry
        </button>
      )}
    </div>
  );
}

function Offer({ transfer: t, onError }: { transfer: Transfer; onError: (text: string) => void }) {
  const [busy, setBusy] = useState(false);
  const streamer = useStreamer((s) => s.active);
  const who = shownName(t.friendName, streamer);
  const run = (action: Promise<void>) => {
    setBusy(true);
    action.catch((err) => onError(String(err))).finally(() => setBusy(false));
  };
  const share = t.size > 0 ? t.moved / t.size : 0;
  const what = t.screenshot ? "a screenshot" : t.recording ? "a recording" : "a clip";

  return (
    <div className="cb-glass rounded-[18px] px-4 py-3.5">
      <p className="truncate text-[13px] font-semibold">
        {t.stage === "asking" ? `${who} wants to send you ${what}` : `Receiving from ${streamer ? "a friend" : who}…`}
      </p>
      <p className="truncate text-xs text-ink-muted">
        {t.name} · {megabytes(t.size)}
        {t.game && ` · ${t.game}`}
      </p>
      {t.stage === "asking" ? (
        <div className="mt-3 flex justify-end gap-2">
          <button
            disabled={busy}
            onClick={() => run(shareApi.decline(t.id))}
            className="rounded-pill px-3.5 py-1.5 text-xs font-semibold text-ink-muted transition-colors hover:bg-white/10 hover:text-ink"
          >
            Decline
          </button>
          <button
            disabled={busy}
            onClick={() => run(shareApi.accept(t.id))}
            className={cn(
              "rounded-pill bg-white px-3.5 py-1.5 text-xs font-semibold text-black transition-opacity",
              busy && "opacity-60",
            )}
          >
            Accept
          </button>
        </div>
      ) : (
        <div className="mt-2.5 flex items-center gap-3">
          <div className="h-1 flex-1 overflow-hidden rounded-pill bg-white/10">
            <div
              className="h-full rounded-pill bg-accent transition-[width] duration-200"
              style={{ width: `${Math.min(Math.max(share, 0), 1) * 100}%` }}
            />
          </div>
          <span className="text-xs text-ink-muted tabular-nums">{Math.floor(share * 100)} %</span>
        </div>
      )}
    </div>
  );
}
