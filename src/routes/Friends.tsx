import { useEffect, useState, type FormEvent, type MouseEvent } from "react";
import { cn } from "@/lib/cn";
import { formatCode, friendsApi, lastSeenText, playingFor, useFriends } from "@/lib/friends";
import type { FriendPending, FriendPresence, FriendUser } from "@/lib/types";
import { useEngine } from "@/store";
import { Button } from "@/components/ui/Button";
import { Card, SectionTitle } from "@/components/ui/Card";
import { useMenu } from "@/components/ui/Menu";
import { useShallow } from "zustand/react/shallow";
import { IconCheck, IconClose, IconCopy, IconGamepad, IconHeart, IconPlus } from "@/components/icons";

/** Shows what the core answered — a refusal from the server reads as it is. */
function report(error: unknown) {
  const message = typeof error === "string" ? error : "Something went wrong.";
  window.dispatchEvent(new CustomEvent("cb-friends-error", { detail: message }));
}

export function Friends() {
  const signedIn = useFriends((s) => s.signedIn);
  return (
    <div className="space-y-8 pb-12">
      <header className="pt-10">
        <h1 className="display text-4xl">Friends</h1>
        <p className="mt-2 text-sm text-ink-muted">
          See who is online and what they are playing.
        </p>
      </header>
      {signedIn ? <SignedIn /> : <SignIn />}
    </div>
  );
}

// --- Signed out ----------------------------------------------------------------

function SignIn() {
  const signingIn = useFriends((s) => s.signingIn);
  const [error, setError] = useState<string | null>(null);

  const start = () => {
    setError(null);
    friendsApi.signIn().catch((err) => setError(String(err)));
  };

  return (
    <Card className="mx-auto flex max-w-md flex-col items-center gap-5 px-8 py-10 text-center">
      <span className="grid h-14 w-14 place-items-center rounded-pill bg-[#5865f2]/15 text-[#8891f7]">
        <DiscordGlyph className="h-7 w-7" />
      </span>
      {signingIn ? (
        <>
          <div className="space-y-1.5">
            <p className="text-lg font-semibold">Waiting for Discord…</p>
            <p className="text-sm text-ink-muted">
              Finish signing in in your browser. ClippiBoy picks it up by itself.
            </p>
          </div>
          <div className="flex gap-2">
            <Button variant="ghost" onClick={() => void friendsApi.cancelSignIn()}>
              Cancel
            </Button>
            <Button onClick={start}>Open the browser again</Button>
          </div>
        </>
      ) : (
        <>
          <div className="space-y-1.5">
            <p className="text-lg font-semibold">Play together, see it live</p>
            <p className="text-sm text-ink-muted">
              Sign in with Discord to add friends, see who is online and which game they
              are in. ClippiBoy only reads your Discord name and picture.
            </p>
          </div>
          <Button variant="primary" onClick={start} icon={<DiscordGlyph className="h-4 w-4" />}>
            Sign in with Discord
          </Button>
        </>
      )}
      {error && <p className="text-sm text-live">{error}</p>}
    </Card>
  );
}

// --- Signed in -----------------------------------------------------------------

function SignedIn() {
  const lists = useFriends((s) => s.lists);
  const presence = useFriends((s) => s.presence);
  const lastSeen = useFriends((s) => s.lastSeen);
  const connected = useFriends((s) => s.connected);
  const favorites = useEngine((s) => s.config.friends.favorites);
  const [error, setError] = useState<string | null>(null);
  const now = useNow();

  useEffect(() => {
    const onError = (event: Event) => setError((event as CustomEvent<string>).detail);
    window.addEventListener("cb-friends-error", onError);
    return () => window.removeEventListener("cb-friends-error", onError);
  }, []);

  // Favourites first in every group; below that the server's alphabetical
  // order, and offline the ones seen most recently.
  const favorite = (a: FriendUser, b: FriendUser) =>
    Number(favorites.includes(b.id)) - Number(favorites.includes(a.id));
  const sorted = [...lists.friends].sort(favorite);
  const playing = sorted.filter((f) => presence[f.id]?.game);
  const online = sorted.filter((f) => presence[f.id] && !presence[f.id].game);
  const offline = sorted
    .filter((f) => !presence[f.id])
    .sort((a, b) => favorite(a, b) || (lastSeen[b.id] ?? 0) - (lastSeen[a.id] ?? 0));

  return (
    <>
      <div className="grid gap-4 md:grid-cols-[minmax(0,1fr)_minmax(0,1.2fr)]">
        <Profile connected={connected} />
        <AddFriend />
      </div>

      {error && (
        <Card className="flex items-center justify-between gap-4 border-live/40 bg-live/8 p-4 text-sm text-live">
          {error}
          <button aria-label="Dismiss" onClick={() => setError(null)}>
            <IconClose className="h-4 w-4" />
          </button>
        </Card>
      )}

      {(lists.incoming.length > 0 || lists.outgoing.length > 0) && (
        <section>
          <SectionTitle title="Requests" />
          <Card className="divide-y divide-line">
            {lists.incoming.map((request) => (
              <RequestRow key={request.id} request={request} incoming />
            ))}
            {lists.outgoing.map((request) => (
              <RequestRow key={request.id} request={request} incoming={false} />
            ))}
          </Card>
        </section>
      )}

      {lists.friends.length === 0 ? (
        <Card className="px-8 py-12 text-center text-sm text-ink-muted">
          No friends yet. Send them your code, or add them by their Discord name.
        </Card>
      ) : (
        <>
          <Group title="In game" friends={playing} presence={presence} lastSeen={lastSeen} now={now} />
          <Group title="Online" friends={online} presence={presence} lastSeen={lastSeen} now={now} />
          <Group title="Offline" friends={offline} presence={presence} lastSeen={lastSeen} now={now} />
        </>
      )}
    </>
  );
}

function Profile({ connected }: { connected: boolean }) {
  const me = useFriends((s) => s.me);
  const { config, patchConfig } = useEngine(useShallow((s) => ({ config: s.config.friends, patchConfig: s.patchConfig })));
  const invisible = config.invisible;
  const [copied, setCopied] = useState(false);
  const [status, setStatus] = useState(config.status);
  useEffect(() => setStatus(config.status), [config.status]);
  if (!me) return <Card className="h-[112px] animate-pulse" />;

  const saveStatus = () => {
    const text = status.trim().slice(0, 60);
    if (text !== config.status) void patchConfig({ friends: { ...config, status: text } });
  };
  const presenceStatus: Status = !connected ? "offline" : invisible ? "invisible" : config.busy ? "busy" : "online";

  const copy = () => {
    void navigator.clipboard.writeText(formatCode(me.friendCode)).then(() => {
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    });
  };

  return (
    <Card className="flex items-center gap-4 p-5">
      <Avatar user={me} status={presenceStatus} size="lg" />
      <div className="min-w-0 flex-1">
        <p className="flex items-center gap-2">
          <span className="truncate font-semibold">{me.displayName}</span>
          <button
            onClick={() => void patchConfig({ friends: { ...config, busy: !config.busy } })}
            title={config.busy ? "Busy: no notices, clips are turned down" : "Set yourself busy"}
            className={cn(
              "shrink-0 rounded-pill border px-2 py-0.5 text-[11px] transition-colors",
              config.busy ? "border-live/50 bg-live/10 text-live" : "border-line text-ink-muted hover:bg-hover",
            )}
          >
            {config.busy ? "Busy" : "Available"}
          </button>
        </p>
        <input
          value={status}
          maxLength={60}
          onChange={(e) => setStatus(e.target.value)}
          onBlur={saveStatus}
          onKeyDown={(e) => {
            if (e.key === "Enter") (e.target as HTMLInputElement).blur();
          }}
          placeholder={!connected ? "Connecting…" : invisible ? "Invisible — you look offline" : "What are you up to?"}
          aria-label="Your status"
          className="mt-0.5 w-full truncate bg-transparent text-xs text-ink-muted outline-none placeholder:text-ink-faint
            focus:text-ink"
        />
      </div>
      <button
        onClick={copy}
        title="Copy your friend code"
        className="flex shrink-0 items-center gap-2 rounded-pill border border-line bg-elevated px-3.5 py-2
          font-mono text-sm tracking-wider transition-colors hover:bg-hover"
      >
        {formatCode(me.friendCode)}
        {copied ? <IconCheck className="h-4 w-4 text-ok" /> : <IconCopy className="h-4 w-4 text-ink-muted" />}
      </button>
    </Card>
  );
}

function AddFriend() {
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState(false);
  const [answer, setAnswer] = useState<{ ok: boolean; text: string } | null>(null);

  const submit = (event: FormEvent) => {
    event.preventDefault();
    const text = query.trim();
    if (!text || busy) return;
    setBusy(true);
    setAnswer(null);
    friendsApi
      .request(text)
      .then((status) => {
        setQuery("");
        setAnswer({
          ok: true,
          text: status === "accepted" ? "They had asked you already — you are friends now." : "Request sent.",
        });
      })
      .catch((err) => setAnswer({ ok: false, text: String(err) }))
      .finally(() => setBusy(false));
  };

  return (
    <Card className="flex flex-col justify-center gap-3 p-5">
      <form onSubmit={submit} className="flex gap-2">
        <input
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setAnswer(null);
          }}
          placeholder="Discord name or friend code"
          aria-label="Discord name or friend code"
          className="h-10 min-w-0 flex-1 rounded-pill border border-line bg-elevated px-4 text-sm
            outline-none transition-colors placeholder:text-ink-faint focus:border-line-strong"
        />
        <Button type="submit" variant="primary" disabled={!query.trim() || busy} icon={<IconPlus className="h-4 w-4" />}>
          Add
        </Button>
      </form>
      <p className={cn("px-1 text-xs", answer ? (answer.ok ? "text-ok" : "text-live") : "text-ink-faint")}>
        {answer?.text ?? "They need ClippiBoy and a sign-in with Discord too."}
      </p>
    </Card>
  );
}

function RequestRow({ request, incoming }: { request: FriendPending; incoming: boolean }) {
  const [busy, setBusy] = useState(false);
  const run = (action: Promise<void>) => {
    setBusy(true);
    action.catch(report).finally(() => setBusy(false));
  };
  return (
    <div className="flex items-center gap-4 px-5 py-3.5">
      <Avatar user={request} />
      <div className="min-w-0 flex-1">
        <p className="truncate text-sm font-medium">{request.displayName}</p>
        <p className="truncate text-xs text-ink-muted">
          {incoming ? "wants to be your friend" : "waiting for an answer"}
        </p>
      </div>
      {incoming ? (
        <div className="flex gap-2">
          <Button size="sm" variant="ghost" disabled={busy} onClick={() => run(friendsApi.remove(request.id))}>
            Decline
          </Button>
          <Button size="sm" variant="primary" disabled={busy} onClick={() => run(friendsApi.accept(request.id))}>
            Accept
          </Button>
        </div>
      ) : (
        <Button size="sm" variant="ghost" disabled={busy} onClick={() => run(friendsApi.remove(request.id))}>
          Withdraw
        </Button>
      )}
    </div>
  );
}

function Group({
  title,
  friends,
  presence,
  lastSeen,
  now,
}: {
  title: string;
  friends: FriendUser[];
  presence: Record<string, FriendPresence>;
  lastSeen: Record<string, number>;
  now: number;
}) {
  if (friends.length === 0) return null;
  return (
    <section>
      <SectionTitle
        title={title}
        action={<span className="pb-1 text-sm text-ink-faint tabular-nums">{friends.length}</span>}
      />
      <Card className="divide-y divide-line">
        {friends.map((friend) => (
          <FriendRow
            key={friend.id}
            friend={friend}
            presence={presence[friend.id] ?? null}
            lastSeen={lastSeen[friend.id]}
            now={now}
          />
        ))}
      </Card>
    </section>
  );
}

function FriendRow({
  friend,
  presence,
  lastSeen,
  now,
}: {
  friend: FriendUser;
  presence: FriendPresence | null;
  lastSeen: number | undefined;
  now: number;
}) {
  const menu = useMenu();
  const { config, patchConfig } = useEngine(useShallow((s) => ({ config: s.config.friends, patchConfig: s.patchConfig })));
  const favorite = config.favorites.includes(friend.id);
  const status: Status = !presence ? "offline" : presence.busy ? "busy" : presence.game ? "playing" : "online";
  const duration = presence?.game ? playingFor(presence.since, now) : null;

  const toggleFavorite = () =>
    void patchConfig({
      friends: {
        ...config,
        favorites: favorite ? config.favorites.filter((id) => id !== friend.id) : [...config.favorites, friend.id],
      },
    });

  const openMenu = (event: MouseEvent) =>
    menu.open(event, [
      {
        kind: "item",
        label: favorite ? "Remove from favorites" : "Add to favorites",
        icon: <IconHeart filled={favorite} className="h-4 w-4" />,
        onSelect: toggleFavorite,
      },
      {
        kind: "item",
        label: "Copy Discord name",
        icon: <IconCopy className="h-4 w-4" />,
        onSelect: () => void navigator.clipboard.writeText(friend.username),
      },
      { kind: "separator" },
      {
        kind: "item",
        label: "Remove friend",
        icon: <IconClose className="h-4 w-4" />,
        onSelect: () => void friendsApi.remove(friend.id).catch(report),
      },
      {
        kind: "item",
        label: "Block",
        danger: true,
        onSelect: () => void friendsApi.block(friend.id).catch(report),
      },
    ]);

  return (
    <div
      onContextMenu={openMenu}
      className={cn("group flex items-center gap-4 px-5 py-3.5", status === "offline" && "opacity-60")}
    >
      <Avatar user={friend} status={status} />
      <div className="min-w-0 flex-1">
        <p className="flex min-w-0 items-center gap-1.5 text-sm font-medium">
          <span className="truncate">{friend.displayName}</span>
          {favorite && <IconHeart filled className="h-3.5 w-3.5 shrink-0 text-accent-bright" />}
          {presence?.status && (
            <span className="truncate text-xs font-normal text-ink-muted">— {presence.status}</span>
          )}
        </p>
        {presence?.game ? (
          <p className="flex min-w-0 items-center gap-1.5 text-xs text-accent-bright">
            <IconGamepad className="h-3.5 w-3.5 shrink-0" />
            <span className="truncate">{presence.game}</span>
            {duration && <span className="shrink-0 text-ink-muted">· {duration}</span>}
            {presence.busy && <span className="shrink-0 text-live">· Busy</span>}
          </p>
        ) : (
          <p className={cn("truncate text-xs", status === "busy" ? "text-live" : "text-ink-muted")}>
            {status === "busy" ? "Busy" : status === "online" ? "Online" : lastSeenText(lastSeen, now)}
          </p>
        )}
      </div>
      <button
        aria-label={`More for ${friend.displayName}`}
        onClick={openMenu}
        className="grid h-8 w-8 place-items-center rounded-pill text-ink-muted opacity-0 transition
          hover:bg-hover hover:text-ink group-hover:opacity-100 focus-visible:opacity-100"
      >
        <svg viewBox="0 0 24 24" className="h-4 w-4" fill="currentColor">
          <circle cx="5" cy="12" r="1.8" />
          <circle cx="12" cy="12" r="1.8" />
          <circle cx="19" cy="12" r="1.8" />
        </svg>
      </button>
    </div>
  );
}

// --- Bits ----------------------------------------------------------------------

type Status = "playing" | "online" | "busy" | "offline" | "invisible";

export function Avatar({ user, status, size = "md" }: { user: FriendUser; status?: Status; size?: "md" | "lg" }) {
  const [broken, setBroken] = useState(false);
  const box = size === "lg" ? "h-14 w-14" : "h-10 w-10";
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
        <span className={cn("grid place-items-center rounded-pill bg-elevated font-semibold text-ink-muted", box)}>
          {user.displayName.slice(0, 1).toUpperCase()}
        </span>
      )}
      {status && (
        <span
          className={cn(
            "absolute -right-0.5 -bottom-0.5 rounded-pill border-[3px] border-surface",
            size === "lg" ? "h-4.5 w-4.5" : "h-3.5 w-3.5",
            status === "playing" && "bg-accent",
            status === "online" && "bg-ok",
            status === "busy" && "bg-live",
            status === "offline" && "bg-ink-faint",
            status === "invisible" && "bg-surface ring-2 ring-inset ring-ink-faint",
          )}
        />
      )}
    </span>
  );
}

/** The clock for "42 min" — a minute is fine enough. */
function useNow(): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(timer);
  }, []);
  return now;
}

function DiscordGlyph({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" className={className} fill="currentColor" aria-hidden>
      <path d="M19.3 5.3A17 17 0 0 0 15 4l-.5 1a15.7 15.7 0 0 0-5 0L9 4a17 17 0 0 0-4.3 1.3C2 9.3 1.3 13.2 1.6 17a17 17 0 0 0 5.3 2.7l1.1-1.8c-.6-.2-1.2-.5-1.8-.9l.4-.3a12.2 12.2 0 0 0 10.6 0l.4.3c-.6.4-1.2.7-1.8.9l1.1 1.8a17 17 0 0 0 5.3-2.7c.4-4.4-.7-8.3-2.9-11.7ZM8.7 14.7c-1 0-1.9-1-1.9-2.1s.8-2.1 1.9-2.1 1.9 1 1.9 2.1-.8 2.1-1.9 2.1Zm6.6 0c-1 0-1.9-1-1.9-2.1s.8-2.1 1.9-2.1 1.9 1 1.9 2.1-.8 2.1-1.9 2.1Z" />
    </svg>
  );
}
