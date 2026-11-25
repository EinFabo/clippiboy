import { useEffect, useRef, useState } from "react";
import { UploadCard, useLinkUploads } from "./Links";
import { createPortal } from "react-dom";
import { cn } from "@/lib/cn";
import { canReceive, lastSeenText, playingFor, useFriends } from "@/lib/friends";
import { friendStatus } from "@/lib/friendsList";
import { clipName, formatDuration, formatSize } from "@/lib/format";
import { fileUrl } from "@/lib/ipc";
import {
  filterClips,
  friendPickRows,
  pickSummary,
  pickedClips,
  pickerChips,
  togglePick,
  type ClipFilter,
} from "@/lib/sendPicker";
import { awaitsAnswer, retrying, waitingFor } from "@/lib/sendQueue";
import {
  isOver,
  megabytes,
  retryTransfers,
  sendClips,
  shareApi,
  useSendQueue,
  useShare,
  useTransferMemory,
  watchTransfers,
} from "@/lib/share";
import { cardLeft, cardStack, noun, onFriendsPage, transferKind, type Ended } from "@/lib/transferRows";
import type { Clip, FriendPresence, FriendUser, Transfer } from "@/lib/types";
import { useEngine } from "@/store";
import { Avatar } from "@/routes/Friends";
import { Button } from "@/components/ui/Button";
import { Pill } from "@/components/ui/Card";
import { ProgressBar } from "@/components/ui/ProgressBar";
import { IconCheck, IconClose, IconGamepad, IconHeart, IconRetry, IconSend } from "@/components/icons";

/** The dim behind both pickers and their panel. */
const DIM = "fixed inset-0 z-50 grid place-items-center bg-black/60 backdrop-blur-sm";
const PANEL = "rounded-card border border-line bg-surface px-6 py-[22px] shadow-2xl";

/** Escape closes a picker while it is open — and only the picker: it is taken
 *  before the player or the gallery underneath get to it. */
function useEscape(open: boolean, close: () => void) {
  useEffect(() => {
    if (!open) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      close();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open, close]);
}

/** Under a friend's name: what they play, or how they are. */
function PresenceLine({
  presence,
  lastSeen,
  now,
  withTime = false,
}: {
  presence: FriendPresence | undefined;
  lastSeen: number | undefined;
  now: number;
  /** "· 42 min" after the game. */
  withTime?: boolean;
}) {
  const line = "mt-0.5 flex min-w-0 items-center gap-1.5 text-xs";
  if (presence?.game) {
    const duration = withTime ? playingFor(presence.since, now) : null;
    return (
      <span className={cn(line, "text-accent-bright")}>
        <IconGamepad className="h-3.5 w-3.5 shrink-0" />
        <span className="truncate">{presence.game}</span>
        {duration && <span className="shrink-0 text-ink-muted">· {duration}</span>}
        {presence.busy && <span className="shrink-0 text-live">· Busy</span>}
      </span>
    );
  }
  if (presence?.busy) return <span className={cn(line, "text-live")}>Busy</span>;
  return (
    <span className={cn(line, "text-ink-muted")}>
      <span className="truncate">{presence ? (presence.status ?? "Online") : lastSeenText(lastSeen, now)}</span>
    </span>
  );
}

/**
 * "Send to a friend" from a clip: everyone, in the Friends page's order. The
 * busy and offline ones stand greyed, so nobody seems to have gone missing.
 * Clips only go across while both are online — like Quick Share, there is no
 * mailbox in between.
 */
export function SharePicker() {
  const clip = useShare((s) => s.picking);
  const pick = useShare((s) => s.pick);
  const transfers = useShare((s) => s.transfers);
  const queue = useSendQueue((s) => s.queue);
  const { friends } = useFriends((s) => s.lists);
  const presence = useFriends((s) => s.presence);
  const lastSeen = useFriends((s) => s.lastSeen);
  const signedIn = useFriends((s) => s.signedIn);
  const favorites = useEngine((s) => s.config.friends.favorites);
  const [error, setError] = useState<string | null>(null);
  const [sending, setSending] = useState<string | null>(null);
  const close = () => pick(null);

  useEffect(() => {
    setError(null);
    setSending(null);
  }, [clip]);
  useEscape(clip !== null, () => pick(null));

  if (!clip) return null;

  const now = Date.now();
  const rows = friendPickRows(friends, presence, lastSeen, favorites);

  const send = (friendId: string) => {
    // Something for them still waits for an answer: this one goes behind it.
    if (awaitsAnswer(transfers, friendId) || waitingFor(queue, friendId).length > 0) {
      sendClips(friendId, [clip]);
      close();
      return;
    }
    setSending(friendId);
    setError(null);
    shareApi
      .send(clip.id, friendId)
      .then(close)
      .catch((err) => {
        setError(String(err));
        setSending(null);
      });
  };

  const row = (friend: FriendUser, reachable: boolean) => (
    <button
      key={friend.id}
      disabled={!reachable || sending !== null}
      onClick={() => send(friend.id)}
      className={cn(
        "group flex w-full items-center gap-3 rounded-inner px-2.5 py-2 text-left transition-colors",
        "hover:bg-hover disabled:cursor-default disabled:hover:bg-transparent",
        !reachable && "opacity-45",
        sending === friend.id && "bg-hover",
      )}
    >
      <Avatar user={friend} status={friendStatus(presence[friend.id])} />
      <span className="flex min-w-0 flex-1 flex-col">
        <span className="flex min-w-0 items-center gap-1.5 text-sm font-medium">
          <span className="truncate">{friend.displayName}</span>
          {favorites.includes(friend.id) && <IconHeart filled className="h-3.5 w-3.5 shrink-0 text-accent-bright" />}
        </span>
        <PresenceLine presence={presence[friend.id]} lastSeen={lastSeen[friend.id]} now={now} />
      </span>
      {sending === friend.id ? (
        <span className="text-xs text-ink-muted">Asking…</span>
      ) : (
        reachable && (
          <span
            aria-hidden
            className="grid h-8 w-8 shrink-0 place-items-center rounded-pill bg-elevated text-ink opacity-0 transition-opacity
              group-hover:opacity-100 group-focus-visible:opacity-100"
          >
            <IconSend className="h-[15px] w-[15px]" />
          </span>
        )
      )}
    </button>
  );

  return createPortal(
    <div className={DIM} onClick={close}>
      <div className={cn("w-[440px]", PANEL)} onClick={(event) => event.stopPropagation()}>
        <div className="flex items-center gap-3">
          <span className="relative block h-[54px] w-24 shrink-0 overflow-hidden rounded-[8px] bg-gradient-to-br from-accent-deep/40 to-black">
            {clip.thumbPath && (
              <img
                src={`${fileUrl(clip.thumbPath)}?v=${clip.sizeBytes}`}
                alt=""
                draggable={false}
                className="h-full w-full object-cover"
              />
            )}
          </span>
          <div className="min-w-0">
            <h2 data-tauri-drag-region className="truncate text-[16px] font-semibold">
              {clipName(clip)}
            </h2>
            <p className="mt-0.5 truncate text-xs text-ink-muted">
              {clip.screenshot ? "Screenshot" : formatDuration(clip.durationMs)} · {formatSize(clip.sizeBytes)}
              {clip.game && ` · ${clip.game}`}
            </p>
          </div>
        </div>
        <div className="mx-1 my-2.5 border-t border-line" />

        <div className="max-h-[360px] overflow-y-auto">
          {!signedIn ? (
            <p className="rounded-inner bg-elevated px-4 py-3 text-sm text-ink-muted">Sign in on the Friends page first.</p>
          ) : friends.length === 0 ? (
            <p className="rounded-inner bg-elevated px-4 py-3 text-sm text-ink-muted">No friends yet.</p>
          ) : (
            <>
              {rows.online.map(({ friend, reachable }) => row(friend, reachable))}
              {rows.offline.length > 0 && (
                <>
                  <p className={cn("mx-2.5 mb-1 text-xs text-ink-muted", rows.online.length > 0 && "mt-3")}>Offline</p>
                  {rows.offline.map((friend) => row(friend, false))}
                </>
              )}
            </>
          )}
        </div>

        {error && <p className="mt-3 text-sm text-live">{error}</p>}
        <div className="mt-[18px] flex justify-end">
          <Button variant="ghost" size="sm" onClick={close}>
            Close
          </Button>
        </div>
      </div>
    </div>,
    document.body,
  );
}

/**
 * "Send a clip" from the Friends page: the friend is chosen, now the clips —
 * the recent ones, the ones from the game they are playing, or the favorites,
 * several at once. They go one after another (`sendQueue`), and the friend's
 * row shows how far they are.
 */
export function ClipPicker() {
  const friend = useShare((s) => s.choosingFor);
  const chooseFor = useShare((s) => s.chooseFor);
  const clips = useEngine((s) => s.clips);
  const presence = useFriends((s) => (friend ? s.presence[friend.id] : undefined));
  const lastSeen = useFriends((s) => (friend ? s.lastSeen[friend.id] : undefined));
  const [filter, setFilter] = useState<ClipFilter>("recent");
  const [picked, setPicked] = useState<string[]>([]);
  const close = () => chooseFor(null);

  useEffect(() => {
    setFilter("recent");
    setPicked([]);
  }, [friend]);
  useEscape(friend !== null, () => chooseFor(null));

  if (!friend) return null;

  const now = Date.now();
  const game = presence?.game ?? null;
  const chips = pickerChips(clips, game);
  const active = chips.some((chip) => chip.id === filter) ? filter : "recent";
  const shown = filterClips(clips, active, game);
  const chosen = pickedClips(clips, picked);
  const reachable = canReceive(presence);

  const send = () => {
    sendClips(friend.id, chosen);
    close();
  };

  return createPortal(
    <div className={DIM} onClick={close}>
      <div className={cn("w-[600px]", PANEL)} onClick={(event) => event.stopPropagation()}>
        <div className="flex items-center gap-3">
          <Avatar user={friend} status={friendStatus(presence)} />
          <div className="flex min-w-0 flex-col">
            <h2 data-tauri-drag-region className="truncate text-[16px] font-semibold">
              Send to {friend.displayName}
            </h2>
            <PresenceLine presence={presence} lastSeen={lastSeen} now={now} withTime />
          </div>
        </div>

        {chips.length > 0 && (
          <div className="mt-4 flex gap-1.5">
            {chips.map((chip) => (
              <button
                key={chip.id}
                aria-pressed={chip.id === active}
                onClick={() => setFilter(chip.id)}
                className={cn(
                  "flex h-7 max-w-[200px] items-center gap-1.5 rounded-pill border px-3 text-xs font-medium transition-colors",
                  chip.id === active
                    ? "border-white bg-white text-black"
                    : "border-line bg-surface text-ink-muted hover:border-line-strong hover:text-ink",
                )}
              >
                {chip.id === "favorites" && <IconHeart filled className="h-3 w-3 shrink-0 text-live" />}
                <span className="truncate">{chip.label}</span>
              </button>
            ))}
          </div>
        )}

        {clips.length === 0 ? (
          <p className="mt-4 rounded-inner bg-elevated px-4 py-3 text-sm text-ink-muted">No clips yet.</p>
        ) : (
          // The padding keeps the ring of a picked tile clear of the scroll edge.
          <div className="-mx-1 mt-3 max-h-[min(420px,calc(100vh-300px))] overflow-y-auto p-1">
            <div className="grid grid-cols-3 gap-3">
              {shown.map((clip) => (
                <PickTile
                  key={clip.id}
                  clip={clip}
                  selected={picked.includes(clip.id)}
                  onToggle={() => setPicked((now) => togglePick(now, clip.id))}
                />
              ))}
            </div>
          </div>
        )}

        <div className="mt-[18px] flex items-center gap-2">
          <span className="mr-auto text-[13px] text-ink-muted tabular-nums">{pickSummary(chosen, formatSize)}</span>
          <Button variant="ghost" size="sm" onClick={close}>
            Close
          </Button>
          <Button
            variant="primary"
            size="sm"
            icon={<IconSend className="h-[15px] w-[15px]" />}
            disabled={chosen.length === 0 || !reachable}
            title={
              reachable
                ? undefined
                : presence?.busy
                  ? `${friend.displayName} is busy right now`
                  : `${friend.displayName} isn't online right now`
            }
            onClick={send}
          >
            {chosen.length > 0 ? `Send ${chosen.length}` : "Send"}
          </Button>
        </div>
      </div>
    </div>,
    document.body,
  );
}

/** A gallery tile, smaller: picture, length, name, and the round check of the
 *  gallery's selection. */
function PickTile({ clip, selected, onToggle }: { clip: Clip; selected: boolean; onToggle: () => void }) {
  return (
    <button
      aria-pressed={selected}
      onClick={onToggle}
      className={cn(
        "overflow-hidden rounded-[14px] border bg-elevated text-left transition-colors",
        selected ? "border-accent shadow-[0_0_0_1px_var(--color-accent)]" : "border-line hover:border-line-strong",
      )}
    >
      <span className="relative block aspect-video bg-gradient-to-br from-accent-deep/40 to-black">
        {clip.thumbPath && (
          <img
            src={`${fileUrl(clip.thumbPath)}?v=${clip.sizeBytes}`}
            alt=""
            draggable={false}
            loading="lazy"
            decoding="async"
            className="h-full w-full object-cover"
          />
        )}
        <Pill className="absolute right-2 bottom-2 px-2 py-0.5 text-[11px]">
          {clip.screenshot ? "Screenshot" : formatDuration(clip.durationMs)}
        </Pill>
        <span
          aria-hidden
          className={cn(
            "absolute top-2 right-2 grid h-[22px] w-[22px] place-items-center rounded-pill border-2 text-white",
            selected ? "border-accent bg-accent" : "border-white/75 bg-black/35",
          )}
        >
          {selected && <IconCheck className="h-3 w-3" />}
        </span>
      </span>
      <span className="block truncate px-2.5 py-2 text-xs font-medium">{clipName(clip)}</span>
    </button>
  );
}

/**
 * Offers waiting for an answer and clips on their way, bottom right. On the
 * Friends page these stand in the friend's row instead, so only the ones that
 * have no row there (and link uploads) stay as cards.
 */
export function Transfers({ onFriendsPage: friendsPage = false }: { onFriendsPage?: boolean }) {
  const transfers = useShare((s) => s.transfers);
  const ended = useTransferMemory((s) => s.ended);
  const friends = useFriends((s) => s.lists.friends);
  // The main window keeps the core's list tidy — see `watchTransfers`.
  useEffect(
    () =>
      watchTransfers((id) => useEngine.getState().clips.some((clip) => clip.id === id)),
    [],
  );
  const now = useTicking(transfers, ended);
  const friendIds = new Set(friends.map((f) => f.id));
  // What has ended while its row was on screen was seen there — leaving the
  // page does not bring it back as a card.
  const seenInRow = useRef(new Set<string>());
  if (friendsPage)
    for (const t of transfers) if (transferKind(t) === "ended" && onFriendsPage(t, friendIds)) seenInRow.current.add(t.id);
  for (const id of seenInRow.current) if (!transfers.some((t) => t.id === id)) seenInRow.current.delete(id);
  const shown = transfers.filter(
    (t) => cardLeft(t, ended, now) > 0 && !seenInRow.current.has(t.id) && !(friendsPage && onFriendsPage(t, friendIds)),
  );
  const uploads = useLinkUploads();
  if (shown.length === 0 && uploads.length === 0) return null;
  const { cards, failed, more } = cardStack(shown, ended);

  // Above the player and the dialogs (z-50, portals later in the DOM): a link
  // shared from the player's menu shows its progress there too. Below menus.
  // Over a full view (player, screenshot, setup) it moves up, clear of the
  // controls those keep in their bottom-right corner.
  return (
    <div data-transfers className="fixed right-6 bottom-6 z-[55] flex w-[340px] flex-col gap-2 [body:has([data-fullview])_&]:bottom-36">
      {uploads.map((upload) => (
        <UploadCard key={`link-${upload.clipId}`} upload={upload} />
      ))}
      {cards.map((transfer) => (
        <TransferCard key={transfer.id} transfer={transfer} />
      ))}
      {more > 0 && (
        <div className="flex items-center gap-3 rounded-card border border-line bg-surface/95 py-2 pr-2 pl-4 shadow-2xl backdrop-blur">
          <p className="min-w-0 flex-1 truncate text-xs text-ink-muted">{more} more failed</p>
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              for (const t of failed) void shareApi.dismiss(t.id);
            }}
          >
            Dismiss all
          </Button>
        </div>
      )}
    </div>
  );
}

/** The clock for the cards: renders again when the next ended card is due to go. */
function useTicking(transfers: Transfer[], ended: Record<string, Ended>): number {
  const [, setTick] = useState(0);
  const now = Date.now();
  useEffect(() => {
    const at = Date.now();
    const next = Math.min(...transfers.map((t) => cardLeft(t, ended, at)).filter((left) => left > 0 && left < Infinity));
    if (!Number.isFinite(next)) return;
    const timer = window.setTimeout(() => setTick((n) => n + 1), next + 50);
    return () => window.clearTimeout(timer);
  });
  return now;
}

function TransferCard({ transfer: t }: { transfer: Transfer }) {
  const [busy, setBusy] = useState(false);
  const run = (action: Promise<void>) => {
    setBusy(true);
    action.finally(() => setBusy(false));
  };
  const incoming = t.direction === "in";
  const share = t.size > 0 ? t.moved / t.size : 0;
  const failed = transferKind(t) === "error";
  const canRetry = failed && !incoming && t.clipId !== null;
  // Already queued to go again — a second click would send it twice.
  const queued = useSendQueue((s) => retrying(s.queue, t.id));
  // Like the row's Retry: nothing goes to someone offline or busy.
  const reachable = useFriends((s) => canReceive(s.presence[t.friendId]));

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
        {failed && (
          <button
            aria-label="Dismiss"
            title="Dismiss"
            onClick={() => void shareApi.dismiss(t.id)}
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
      {failed && (t.error || canRetry) && (
        <div className="mt-2 flex items-center gap-3">
          {t.error && <p className="min-w-0 flex-1 text-xs text-live">{t.error}</p>}
          {canRetry && (
            <Button
              size="sm"
              className="ml-auto"
              icon={<IconRetry className="h-3.5 w-3.5" />}
              disabled={queued || !reachable}
              title={reachable ? undefined : "They aren't online right now"}
              onClick={() => retryTransfers([t])}
            >
              Retry
            </Button>
          )}
        </div>
      )}
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
  const what = noun(t);
  const What = what[0].toUpperCase() + what.slice(1);
  if (t.direction === "in") {
    switch (t.stage) {
      case "asking":
        return `${who} wants to send you a ${what}`;
      case "moving":
        return `Receiving from ${who}…`;
      case "done":
        return `From ${who} — in your clips`;
      case "expired":
        return `Missed a ${what} from ${who}`;
      case "declined":
        return `Declined ${who}'s ${what}`;
      case "failed":
        return `${What} from ${who} didn't arrive`;
      default:
        return `${What} from ${who}`;
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
      return `${who} has your ${what}`;
    case "declined":
      return `${who} said no`;
    case "expired":
      return `${who} didn't answer`;
    case "failed":
      return `Couldn't send to ${who}`;
    default:
      return `${What} for ${who}`;
  }
}
