import { useEffect, useState } from "react";
import { cn } from "@/lib/cn";
import { canReceive, friendsApi, playingFor, useFriends } from "@/lib/friends";
import { clipName } from "@/lib/format";
import type { Clip, FriendPresence, FriendUser } from "@/lib/types";
import { IconClose, IconSend } from "@/components/icons";

/**
 * Die Freunde in der Konsole — c01 und c05 aus dem Freunde-Labor: oben offene
 * Anfragen zum Annehmen direkt im Spiel, darunter wer spielt, dann wer nur
 * online ist. Wer offline ist, steht eingeklappt darunter.
 *
 * Kein eigener Kanal: `friends-state` geht an jedes Fenster, und der Store in
 * `lib/friends.ts` hört hier genauso zu wie im Hauptfenster.
 */
export function FriendsPanel({
  latest,
  onSend,
}: {
  /** The newest clip — what "Letzten Clip senden" sends. */
  latest: Clip | null;
  onSend: (clip: Clip, friend: FriendUser) => void;
}) {
  const { signedIn, lists, presence, lastSeen } = useFriends();
  const [showOffline, setShowOffline] = useState(false);
  const now = useNow();

  if (!signedIn) {
    return <p className="text-sm text-ink-muted">Sign in with Discord under Friends in the main window.</p>;
  }

  const playing = lists.friends.filter((f) => presence[f.id]?.game);
  const online = lists.friends.filter((f) => presence[f.id] && !presence[f.id].game);
  const offline = lists.friends
    .filter((f) => !presence[f.id])
    .sort((a, b) => (lastSeen[b.id] ?? 0) - (lastSeen[a.id] ?? 0));

  return (
    <div className="grid gap-1">
      {lists.incoming.map((request) => (
        <Request key={request.id} user={request} />
      ))}
      {lists.friends.length === 0 && lists.incoming.length === 0 && (
        <p className="text-sm text-ink-muted">No friends yet. Add them under Friends in the main window.</p>
      )}
      {[...playing, ...online].map((friend) => (
        <Row
          key={friend.id}
          friend={friend}
          presence={presence[friend.id]}
          now={now}
          latest={latest}
          onSend={onSend}
        />
      ))}
      {offline.length > 0 && (
        <>
          <button
            onClick={() => setShowOffline((v) => !v)}
            className="mt-1 justify-self-start rounded-pill px-3 py-1.5 text-xs text-ink-muted transition-colors hover:bg-white/10 hover:text-ink"
          >
            {showOffline ? "Hide offline" : `${offline.length} offline`}
          </button>
          {showOffline &&
            offline.map((friend) => (
              <Row key={friend.id} friend={friend} presence={null} lastSeen={lastSeen[friend.id]} now={now} />
            ))}
        </>
      )}
    </div>
  );
}

function Request({ user }: { user: FriendUser }) {
  const [busy, setBusy] = useState(false);
  const run = (action: Promise<void>) => {
    setBusy(true);
    action.catch(() => {}).finally(() => setBusy(false));
  };
  return (
    <div className="mb-1 flex items-center gap-3 rounded-[16px] bg-accent/15 px-3 py-2.5">
      <Face user={user} />
      <div className="min-w-0 flex-1">
        <p className="truncate text-[13px] font-semibold">{user.displayName}</p>
        <p className="truncate text-xs text-ink-muted">wants to be your friend</p>
      </div>
      <button
        disabled={busy}
        onClick={() => run(friendsApi.remove(user.id))}
        className="rounded-pill bg-white/10 px-3 py-1.5 text-xs font-semibold transition-colors hover:bg-white/15"
      >
        Decline
      </button>
      <button
        disabled={busy}
        onClick={() => run(friendsApi.accept(user.id))}
        className="rounded-pill bg-white px-3 py-1.5 text-xs font-semibold text-black"
      >
        Accept
      </button>
    </div>
  );
}

function Row({
  friend,
  presence,
  lastSeen,
  now,
  latest,
  onSend,
}: {
  friend: FriendUser;
  presence: FriendPresence | null;
  lastSeen?: number;
  now: number;
  latest?: Clip | null;
  onSend?: (clip: Clip, friend: FriendUser) => void;
}) {
  const duration = presence?.game ? playingFor(presence.since, now) : null;
  const line = !presence
    ? zuletzt(lastSeen, now)
    : presence.game
      ? `${presence.game}${duration ? ` · ${duration}` : ""}`
      : presence.busy
        ? "Busy"
        : (presence.status ?? "Online");
  return (
    <div className={cn("flex items-center gap-3 rounded-[14px] px-2 py-2", !presence && "opacity-60")}>
      <Face user={friend} status={!presence ? "off" : presence.busy ? "busy" : presence.game ? "game" : "on"} />
      <div className="min-w-0 flex-1">
        <p className="truncate text-[13px] font-semibold">{friend.displayName}</p>
        <p className={cn("truncate text-xs", presence?.game ? "text-accent-bright" : "text-ink-muted")}>{line}</p>
      </div>
      {latest && onSend && canReceive(presence) && (
        <button
          title={`Send latest clip: ${clipName(latest)}`}
          onClick={() => onSend(latest, friend)}
          className="flex shrink-0 items-center gap-1.5 rounded-pill bg-white/10 px-3 py-1.5 text-xs font-semibold transition-colors hover:bg-white/15"
        >
          <IconSend className="h-3.5 w-3.5" />
          Latest clip
        </button>
      )}
    </div>
  );
}

/**
 * Über einer Clip-Kachel: an wen soll er gehen? Nur wer online und nicht
 * beschäftigt ist — ohne Postfach dazwischen kann niemand sonst annehmen.
 */
export function SendTo({ clip, onSend, onClose }: { clip: Clip; onSend: (clip: Clip, friend: FriendUser) => void; onClose: () => void }) {
  const { signedIn, lists, presence } = useFriends();
  const reachable = lists.friends.filter((f) => canReceive(presence[f.id]));
  return (
    <div className="absolute inset-0 z-10 flex flex-col rounded-inner bg-black/85 p-2 backdrop-blur-md">
      <div className="flex items-center justify-between px-1 pb-1">
        <span className="text-xs font-semibold text-ink-muted">Send to</span>
        <button
          aria-label="Close"
          onClick={onClose}
          className="grid h-6 w-6 place-items-center rounded-pill text-ink-muted transition-colors hover:bg-white/10 hover:text-ink"
        >
          <IconClose className="h-3 w-3" />
        </button>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto">
        {!signedIn ? (
          <p className="px-1 text-xs text-ink-muted">Sign in under Friends in the main window first.</p>
        ) : reachable.length === 0 ? (
          <p className="px-1 text-xs text-ink-muted">No friend is online right now.</p>
        ) : (
          reachable.map((friend) => (
            <button
              key={friend.id}
              onClick={() => onSend(clip, friend)}
              className="flex w-full items-center gap-2 rounded-[10px] px-1.5 py-1 text-left transition-colors hover:bg-white/10"
            >
              <Face user={friend} size="sm" />
              <span className="min-w-0 flex-1 truncate text-xs font-semibold">{friend.displayName}</span>
              <IconSend className="h-3.5 w-3.5 shrink-0 text-ink-muted" />
            </button>
          ))
        )}
      </div>
    </div>
  );
}

function Face({
  user,
  status,
  size = "md",
}: {
  user: FriendUser;
  status?: "on" | "game" | "busy" | "off";
  size?: "sm" | "md";
}) {
  const [broken, setBroken] = useState(false);
  const box = size === "sm" ? "h-6 w-6" : "h-9 w-9";
  return (
    <span className={cn("relative shrink-0", box)}>
      {user.avatar && !broken ? (
        <img
          src={user.avatar}
          alt=""
          draggable={false}
          onError={() => setBroken(true)}
          className={cn("rounded-pill object-cover", box)}
        />
      ) : (
        <span className={cn("grid place-items-center rounded-pill bg-white/10 font-bold text-ink-muted", box, size === "sm" ? "text-[10px]" : "text-sm")}>
          {user.displayName.slice(0, 1).toUpperCase()}
        </span>
      )}
      {status && (
        <span
          className={cn(
            "absolute -right-0.5 -bottom-0.5 h-3 w-3 rounded-pill border-2 border-[#07070a]",
            status === "on" && "bg-ok",
            status === "game" && "bg-accent",
            status === "busy" && "bg-live",
            status === "off" && "bg-ink-faint",
          )}
        />
      )}
    </span>
  );
}

/** „zuletzt vor 5 min", „… gestern", „… vor 3 Tagen" — `lastSeenText` auf Deutsch. */
function zuletzt(at: number | undefined, now: number): string {
  if (at === undefined) return "Offline";
  const minutes = Math.max(0, Math.floor((now - at) / 60_000));
  if (minutes < 1) return "just left";
  if (minutes < 60) return `seen ${minutes} min ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `seen ${hours} h ago`;
  const days = Math.floor(hours / 24);
  return days === 1 ? "seen yesterday" : `seen ${days} days ago`;
}

/** Die Uhr für „42 min" — eine halbe Minute genügt. */
function useNow(): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(timer);
  }, []);
  return now;
}
