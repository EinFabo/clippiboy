import { useEffect, useState } from "react";
import { cn } from "@/lib/cn";
import { friendsApi, playingFor, useFriends } from "@/lib/friends";
import type { FriendPresence, FriendUser } from "@/lib/types";

/**
 * Die Freunde in der Konsole — c01 und c05 aus dem Freunde-Labor: oben offene
 * Anfragen zum Annehmen direkt im Spiel, darunter wer spielt, dann wer nur
 * online ist. Wer offline ist, steht eingeklappt darunter.
 *
 * Kein eigener Kanal: `friends-state` geht an jedes Fenster, und der Store in
 * `lib/friends.ts` hört hier genauso zu wie im Hauptfenster.
 */
export function FriendsPanel() {
  const { signedIn, lists, presence, lastSeen } = useFriends();
  const [showOffline, setShowOffline] = useState(false);
  const now = useNow();

  if (!signedIn) {
    return <p className="text-sm text-ink-muted">Im Fenster unter Freunde mit Discord anmelden.</p>;
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
        <p className="text-sm text-ink-muted">Noch keine Freunde. Hinzufügen geht im Fenster unter Freunde.</p>
      )}
      {[...playing, ...online].map((friend) => (
        <Row key={friend.id} friend={friend} presence={presence[friend.id]} now={now} />
      ))}
      {offline.length > 0 && (
        <>
          <button
            onClick={() => setShowOffline((v) => !v)}
            className="mt-1 justify-self-start rounded-pill px-3 py-1.5 text-xs text-ink-muted transition-colors hover:bg-white/10 hover:text-ink"
          >
            {showOffline ? "Offline ausblenden" : `${offline.length} offline`}
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
        <p className="truncate text-xs text-ink-muted">möchte dein Freund sein</p>
      </div>
      <button
        disabled={busy}
        onClick={() => run(friendsApi.remove(user.id))}
        className="rounded-pill bg-white/10 px-3 py-1.5 text-xs font-semibold transition-colors hover:bg-white/15"
      >
        Ablehnen
      </button>
      <button
        disabled={busy}
        onClick={() => run(friendsApi.accept(user.id))}
        className="rounded-pill bg-white px-3 py-1.5 text-xs font-semibold text-black"
      >
        Annehmen
      </button>
    </div>
  );
}

function Row({
  friend,
  presence,
  lastSeen,
  now,
}: {
  friend: FriendUser;
  presence: FriendPresence | null;
  lastSeen?: number;
  now: number;
}) {
  const duration = presence?.game ? playingFor(presence.since, now) : null;
  const line = !presence
    ? zuletzt(lastSeen, now)
    : presence.game
      ? `${presence.game}${duration ? ` · ${duration}` : ""}`
      : presence.busy
        ? "Beschäftigt"
        : (presence.status ?? "Online");
  return (
    <div className={cn("flex items-center gap-3 rounded-[14px] px-2 py-2", !presence && "opacity-60")}>
      <Face user={friend} status={!presence ? "off" : presence.busy ? "busy" : presence.game ? "game" : "on"} />
      <div className="min-w-0 flex-1">
        <p className="truncate text-[13px] font-semibold">{friend.displayName}</p>
        <p className={cn("truncate text-xs", presence?.game ? "text-accent-bright" : "text-ink-muted")}>{line}</p>
      </div>
    </div>
  );
}

function Face({ user, status }: { user: FriendUser; status?: "on" | "game" | "busy" | "off" }) {
  const [broken, setBroken] = useState(false);
  return (
    <span className="relative h-9 w-9 shrink-0">
      {user.avatar && !broken ? (
        <img
          src={user.avatar}
          alt=""
          draggable={false}
          onError={() => setBroken(true)}
          className="h-9 w-9 rounded-pill object-cover"
        />
      ) : (
        <span className="grid h-9 w-9 place-items-center rounded-pill bg-white/10 text-sm font-bold text-ink-muted">
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
  if (minutes < 1) return "gerade eben da gewesen";
  if (minutes < 60) return `zuletzt vor ${minutes} min`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `zuletzt vor ${hours} h`;
  const days = Math.floor(hours / 24);
  return days === 1 ? "zuletzt gestern" : `zuletzt vor ${days} Tagen`;
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
