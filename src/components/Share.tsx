import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { cn } from "@/lib/cn";
import { canReceive, useFriends } from "@/lib/friends";
import { clipName, formatDuration } from "@/lib/format";
import { fileUrl } from "@/lib/ipc";
import { isOver, megabytes, shareApi, useShare } from "@/lib/share";
import type { Transfer } from "@/lib/types";
import { useEngine } from "@/store";
import { Avatar } from "@/routes/Friends";
import { Button } from "@/components/ui/Button";
import { ProgressBar } from "@/components/ui/ProgressBar";
import { IconCheck, IconClose, IconGamepad, IconHeart, IconSend } from "@/components/icons";

/**
 * "Send to a friend": the friends who are online right now, favourites first.
 * Clips only go across while both are online — like Quick Share, there is no
 * mailbox in between.
 */
export function SharePicker() {
  const clip = useShare((s) => s.picking);
  const pick = useShare((s) => s.pick);
  const { friends } = useFriends((s) => s.lists);
  const presence = useFriends((s) => s.presence);
  const signedIn = useFriends((s) => s.signedIn);
  const favorites = useEngine((s) => s.config.friends.favorites);
  const [error, setError] = useState<string | null>(null);
  const [sending, setSending] = useState<string | null>(null);

  useEffect(() => {
    setError(null);
    setSending(null);
  }, [clip]);

  useEffect(() => {
    if (!clip) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") pick(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [clip, pick]);

  if (!clip) return null;

  const online = friends
    .filter((f) => presence[f.id])
    .sort((a, b) => Number(favorites.includes(b.id)) - Number(favorites.includes(a.id)));

  const send = (friendId: string) => {
    setSending(friendId);
    setError(null);
    shareApi
      .send(clip.id, friendId)
      .then(() => pick(null))
      .catch((err) => {
        setError(String(err));
        setSending(null);
      });
  };

  return createPortal(
    <div className="fixed inset-0 z-50 grid place-items-center bg-black/60 backdrop-blur-sm" onClick={() => pick(null)}>
      <div
        className="w-[420px] rounded-card border border-line bg-surface p-6 shadow-2xl"
        onClick={(event) => event.stopPropagation()}
      >
        <h2 data-tauri-drag-region className="text-[16px] font-medium">
          Send to a friend
        </h2>
        <p className="mt-1 text-xs text-ink-muted">
          Goes straight from your PC to theirs, in full quality. They have to be online and say yes.
        </p>

        <div className="mt-5 max-h-[320px] space-y-1.5 overflow-y-auto">
          {!signedIn ? (
            <p className="rounded-inner bg-elevated px-4 py-3 text-sm text-ink-muted">
              Sign in on the Friends page first.
            </p>
          ) : online.length === 0 ? (
            <p className="rounded-inner bg-elevated px-4 py-3 text-sm text-ink-muted">
              None of your friends is online right now.
            </p>
          ) : (
            online.map((friend) => {
              const p = presence[friend.id];
              return (
                <button
                  key={friend.id}
                  disabled={sending !== null || p.busy}
                  onClick={() => send(friend.id)}
                  className={cn(
                    "flex w-full items-center gap-3 rounded-inner border border-line px-3 py-2.5 text-left transition-colors",
                    "hover:bg-hover disabled:cursor-default disabled:opacity-50",
                    sending === friend.id && "border-white/25 bg-elevated",
                  )}
                >
                  <Avatar user={friend} status={p.busy ? "busy" : p.game ? "playing" : "online"} />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-sm font-medium">
                      {favorites.includes(friend.id) && <IconHeart filled className="mr-1 inline h-3 w-3 text-accent-bright" />}
                      {friend.displayName}
                    </span>
                    <span className="flex items-center gap-1.5 truncate text-xs text-ink-muted">
                      {p.busy ? (
                        "Busy"
                      ) : p.game ? (
                        <>
                          <IconGamepad className="h-3.5 w-3.5 shrink-0" />
                          {p.game}
                        </>
                      ) : (
                        (p.status ?? "Online")
                      )}
                    </span>
                  </span>
                  {sending === friend.id && <span className="text-xs text-ink-muted">Asking…</span>}
                </button>
              );
            })
          )}
        </div>

        {error && <p className="mt-3 text-sm text-live">{error}</p>}
        <div className="mt-5 flex justify-end">
          <Button variant="ghost" onClick={() => pick(null)}>
            Close
          </Button>
        </div>
      </div>
    </div>,
    document.body,
  );
}

/** How many clips the friend-first picker offers — the recent ones are what you
 *  send; the rest is a right click in the library away. */
const RECENT_CLIPS = 12;

/**
 * "Send a clip" from the Friends page: the friend is chosen, now the clip. The
 * other way round from [`SharePicker`], same sending underneath.
 */
export function ClipPicker() {
  const friend = useShare((s) => s.choosingFor);
  const chooseFor = useShare((s) => s.chooseFor);
  const clips = useEngine((s) => s.clips);
  const presence = useFriends((s) => (friend ? s.presence[friend.id] : undefined));
  const [error, setError] = useState<string | null>(null);
  const [sending, setSending] = useState<string | null>(null);

  useEffect(() => {
    setError(null);
    setSending(null);
  }, [friend]);

  useEffect(() => {
    if (!friend) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") chooseFor(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [friend, chooseFor]);

  if (!friend) return null;

  const reachable = canReceive(presence);
  const recent = clips.slice(0, RECENT_CLIPS);

  const send = (clipId: string) => {
    setSending(clipId);
    setError(null);
    shareApi
      .send(clipId, friend.id)
      .then(() => chooseFor(null))
      .catch((err) => {
        setError(String(err));
        setSending(null);
      });
  };

  return createPortal(
    <div className="fixed inset-0 z-50 grid place-items-center bg-black/60 backdrop-blur-sm" onClick={() => chooseFor(null)}>
      <div
        className="w-[560px] rounded-card border border-line bg-surface p-6 shadow-2xl"
        onClick={(event) => event.stopPropagation()}
      >
        <div className="flex items-center gap-3">
          <Avatar user={friend} />
          <div className="min-w-0">
            <h2 data-tauri-drag-region className="truncate text-[16px] font-medium">
              Send a clip to {friend.displayName}
            </h2>
            <p className="text-xs text-ink-muted">Straight to their PC, in full quality. They have to say yes.</p>
          </div>
        </div>

        <div className="mt-5 max-h-[380px] overflow-y-auto">
          {!reachable ? (
            <p className="rounded-inner bg-elevated px-4 py-3 text-sm text-ink-muted">
              {presence?.busy ? `${friend.displayName} is busy right now.` : `${friend.displayName} isn't online anymore.`}
            </p>
          ) : recent.length === 0 ? (
            <p className="rounded-inner bg-elevated px-4 py-3 text-sm text-ink-muted">No clips yet.</p>
          ) : (
            <div className="grid grid-cols-3 gap-3">
              {recent.map((clip) => (
                <button
                  key={clip.id}
                  disabled={sending !== null}
                  onClick={() => send(clip.id)}
                  className={cn(
                    "group rounded-inner p-1 text-left transition-colors hover:bg-hover disabled:cursor-default",
                    sending !== null && sending !== clip.id && "opacity-50",
                  )}
                >
                  <span className="relative block aspect-video overflow-hidden rounded-[10px] bg-elevated">
                    {clip.thumbPath && (
                      <img
                        src={`${fileUrl(clip.thumbPath)}?v=${clip.sizeBytes}`}
                        alt=""
                        draggable={false}
                        className="h-full w-full object-cover"
                      />
                    )}
                    <span className="absolute right-1.5 bottom-1.5 rounded-pill bg-black/60 px-1.5 py-0.5 text-[10px] font-semibold">
                      {clip.screenshot ? "Screenshot" : formatDuration(clip.durationMs)}
                    </span>
                    <span className="absolute inset-0 grid place-items-center bg-black/40 opacity-0 transition-opacity group-hover:opacity-100">
                      <IconSend className="h-5 w-5" />
                    </span>
                  </span>
                  <span className="mt-1.5 block truncate text-xs font-medium">
                    {sending === clip.id ? "Asking…" : clipName(clip)}
                  </span>
                </button>
              ))}
            </div>
          )}
        </div>

        {error && <p className="mt-3 text-sm text-live">{error}</p>}
        <div className="mt-5 flex justify-end">
          <Button variant="ghost" onClick={() => chooseFor(null)}>
            Close
          </Button>
        </div>
      </div>
    </div>,
    document.body,
  );
}

/** Offers waiting for an answer and clips on their way, bottom right on every page. */
export function Transfers() {
  const transfers = useShare((s) => s.transfers);
  // Called off by one side or the other — nothing left to show.
  useEffect(() => {
    for (const t of transfers) if (t.stage === "cancelled") void shareApi.dismiss(t.id);
  }, [transfers]);
  const shown = transfers.filter((t) => t.stage !== "cancelled");
  if (shown.length === 0) return null;

  return (
    <div className="fixed right-6 bottom-6 z-40 flex w-[340px] flex-col gap-2">
      {shown.map((transfer) => (
        <TransferCard key={transfer.id} transfer={transfer} />
      ))}
    </div>
  );
}

/** How long a finished card stays: long enough to read, a failure longer. */
function lingerMs(t: Transfer): number {
  return t.stage === "failed" ? 15_000 : 6_000;
}

function TransferCard({ transfer: t }: { transfer: Transfer }) {
  const over = isOver(t);
  // Finished cards go by themselves — they piled up before.
  useEffect(() => {
    if (!over) return;
    const timer = window.setTimeout(() => void shareApi.dismiss(t.id), lingerMs(t));
    return () => window.clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [over, t.id, t.stage]);
  const [busy, setBusy] = useState(false);
  const run = (action: Promise<void>) => {
    setBusy(true);
    action.finally(() => setBusy(false));
  };
  const incoming = t.direction === "in";
  const share = t.size > 0 ? t.moved / t.size : 0;

  return (
    <div className="rounded-card border border-line bg-surface/95 p-4 shadow-2xl backdrop-blur">
      <div className="flex items-start gap-3">
        <div className="min-w-0 flex-1">
          <p className="truncate text-sm font-medium">{headline(t)}</p>
          <p className="truncate text-xs text-ink-muted">
            {t.name} · {megabytes(t.size)}
            {t.game && ` · ${t.game}`}
          </p>
        </div>
        {!isOver(t) && !(incoming && t.stage === "asking") && (
          <button
            aria-label="Cancel"
            title="Cancel"
            onClick={() => void shareApi.cancel(t.id)}
            className="grid h-7 w-7 shrink-0 place-items-center rounded-pill text-ink-muted hover:bg-hover hover:text-ink"
          >
            <IconClose className="h-3.5 w-3.5" />
          </button>
        )}
        {t.stage === "done" && <IconCheck className="h-4 w-4 shrink-0 text-ok" />}
      </div>

      {t.stage === "moving" && (
        <div className="mt-3 flex items-center gap-3">
          <ProgressBar share={share} className="flex-1" />
          <span className="text-xs text-ink-muted tabular-nums">{Math.floor(share * 100)} %</span>
        </div>
      )}
      {t.stage === "failed" && t.error && <p className="mt-2 text-xs text-live">{t.error}</p>}
      {incoming && t.stage === "asking" && (
        <div className="mt-3 flex justify-end gap-2">
          <Button size="sm" variant="ghost" disabled={busy} onClick={() => run(shareApi.decline(t.id))}>
            Decline
          </Button>
          <Button size="sm" variant="primary" disabled={busy} onClick={() => run(shareApi.accept(t.id))}>
            Accept
          </Button>
        </div>
      )}
    </div>
  );
}

function headline(t: Transfer): string {
  const who = t.friendName;
  if (t.direction === "in") {
    switch (t.stage) {
      case "asking":
        return `${who} wants to send you a ${t.screenshot ? "screenshot" : "clip"}`;
      case "moving":
        return `Receiving from ${who}…`;
      case "done":
        return `From ${who} — in your clips`;
      case "expired":
        return `Missed a clip from ${who}`;
      case "declined":
        return `Declined ${who}'s clip`;
      case "failed":
        return `Clip from ${who} didn't arrive`;
      default:
        return `Clip from ${who}`;
    }
  }
  switch (t.stage) {
    case "preparing":
      return `Getting ready for ${who}…`;
    case "asking":
      return `Waiting for ${who} to accept…`;
    case "moving":
      return `Sending to ${who}…`;
    case "done":
      return `${who} has your clip`;
    case "declined":
      return `${who} said no`;
    case "expired":
      return `${who} didn't answer`;
    case "failed":
      return `Couldn't send to ${who}`;
    default:
      return `Clip for ${who}`;
  }
}
