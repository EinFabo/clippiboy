import { useEffect, useRef, useState, type FormEvent, type MouseEvent, type ReactNode, type RefObject } from "react";
import { cn } from "@/lib/cn";
import {
  canReceive,
  cancelSignIn,
  formatCode,
  friendsApi,
  inviteLink,
  lastSeenText,
  playingFor,
  startSignIn,
  useFriends,
  useFriendsExtra,
} from "@/lib/friends";
import {
  STATUS_LABEL,
  addAnswer,
  awayOrder,
  maskedCode,
  myStatus,
  newcomer,
  seenWhileAway,
  shortLink,
  statusChoice,
  statusPatch,
  type AddOutcome,
  type MyStatus,
  type StatusChoice,
} from "@/lib/friendsPage";
import { openSettings } from "@/lib/settingsLink";
import { useStreamer } from "@/lib/streamer";
import { megabytes, openClip, retryTransfers, shareApi, useShare, useTransferMemory } from "@/lib/share";
import { ago, friendStatus, groupFriends, pendingRows, type FriendStatus } from "@/lib/friendsList";
import { noun, rowTransfer, type Ended, type RowTransfer } from "@/lib/transferRows";
import { readOpen, writeOpen } from "@/lib/groupState";
import { formatDuration } from "@/lib/format";
import { toast } from "@/lib/toastBus";
import type { FriendMe, FriendPending, FriendPresence, FriendUser, Transfer } from "@/lib/types";
import { useEngine } from "@/store";
import { Button } from "@/components/ui/Button";
import { Card } from "@/components/ui/Card";
import { ConfirmDelete } from "@/components/ui/ConfirmDelete";
import { ProgressBar } from "@/components/ui/ProgressBar";
import { useMenu } from "@/components/ui/Menu";
import { Popover } from "@/components/ui/Popover";
import { useShallow } from "zustand/react/shallow";
import {
  IconCheck,
  IconClips,
  IconClose,
  IconCopy,
  IconEye,
  IconFriends,
  IconGamepad,
  IconHash,
  IconHeart,
  IconLink,
  IconPlus,
  IconRetry,
  IconSend,
  IconSettings,
} from "@/components/icons";

/** Shows what the core answered — a refusal from the server reads as it is. */
function report(error: unknown) {
  toast("error", typeof error === "string" ? error : "Something went wrong.");
}

/** How long the hub may be away before the page says so — a start or a short
 *  drop only shows "Connecting…" in the chip. */
const AWAY_AFTER_MS = 4000;

export function Friends() {
  const signedIn = useFriends((s) => s.signedIn);
  return <div className="space-y-6 pb-12">{signedIn ? <SignedIn /> : <SignedOut />}</div>;
}

/** Title left, everything else on the right — built like Audio's. */
function Header({ children }: { children?: ReactNode }) {
  return (
    <header className="flex items-center gap-2.5 pt-10">
      <h1 className="display mr-auto text-4xl">Friends</h1>
      {children}
    </header>
  );
}

// --- Signed out ----------------------------------------------------------------

function SignedOut() {
  const signingIn = useFriends((s) => s.signingIn);
  const error = useFriendsExtra((s) => s.signInError);

  return (
    <>
      <Header />
      <div className="relative">
        <GhostList />
        <Card
          className="absolute top-14 left-1/2 flex w-[440px] max-w-full -translate-x-1/2 flex-col items-center gap-3.5
            px-8 pt-9 pb-8 text-center shadow-[0_24px_64px_rgba(0,0,0,0.55)]"
        >
          <span className="grid h-14 w-14 place-items-center rounded-pill bg-[#5865f2]/15 text-[#8891f7]">
            <DiscordGlyph className="h-7 w-7" />
          </span>
          {signingIn ? (
            <>
              <div className="space-y-1">
                <p className="text-lg font-semibold">Waiting for Discord…</p>
                <p className="text-[13px] text-ink-muted">Finish in your browser.</p>
              </div>
              <div className="flex gap-2">
                <Button variant="ghost" onClick={cancelSignIn}>
                  Cancel
                </Button>
                <Button onClick={startSignIn}>Open browser again</Button>
              </div>
            </>
          ) : (
            <>
              <div className="space-y-1">
                <p className="text-lg font-semibold">See who is playing</p>
                <p className="text-[13px] text-ink-muted">ClippiBoy only reads your Discord name and picture.</p>
              </div>
              <Button
                variant="primary"
                onClick={startSignIn}
                icon={error ? <IconRetry className="h-4 w-4" /> : <DiscordGlyph className="h-4 w-4" />}
              >
                {error ? "Try again" : "Sign in with Discord"}
              </Button>
              {error && <p className="text-[13px] text-live">{error}</p>}
            </>
          )}
        </Card>
      </div>
    </>
  );
}

/** What the list looks like once signed in, faint behind the sign-in card. */
const GHOST: { title: string; rows: { name: string; status: Status; line: string; game?: boolean }[] }[] = [
  {
    title: "In game",
    rows: [
      { name: "Alex", status: "playing", line: "Valorant · 1 h 15 min", game: true },
      { name: "Robin", status: "playing", line: "Counter-Strike 2 · 42 min", game: true },
    ],
  },
  {
    title: "Online",
    rows: [
      { name: "Sam", status: "online", line: "Online" },
      { name: "Kim", status: "busy", line: "Busy" },
    ],
  },
  {
    title: "Offline",
    rows: [
      { name: "Chris", status: "offline", line: "Last seen 12 min ago" },
      { name: "Jo", status: "offline", line: "Last seen yesterday" },
      { name: "Max", status: "offline", line: "Last seen 5 days ago" },
    ],
  },
];

function GhostList() {
  return (
    <div aria-hidden inert className="pointer-events-none mt-4 space-y-6 opacity-[0.22] blur-[0.6px] select-none">
      {GHOST.map((group) => (
        <section key={group.title}>
          <GroupTitle title={group.title} count={group.rows.length} />
          <Card className="divide-y divide-line">
            {group.rows.map((row) => (
              <div key={row.name} className={cn("flex items-center gap-4 px-5 py-3.5", row.status === "offline" && "opacity-60")}>
                <Avatar user={{ id: row.name, username: row.name, displayName: row.name, avatar: null }} status={row.status} />
                <div className="min-w-0 flex-1">
                  <p className="text-sm font-medium">{row.name}</p>
                  <p
                    className={cn(
                      "flex items-center gap-1.5 text-xs",
                      row.game ? "text-accent-bright" : row.status === "busy" ? "text-live" : "text-ink-muted",
                    )}
                  >
                    {row.game && <IconGamepad className="h-3.5 w-3.5" />}
                    {row.line}
                  </p>
                </div>
                {row.status !== "offline" && row.status !== "busy" && <IconSend className="h-4 w-4 text-ink-muted" />}
              </div>
            ))}
          </Card>
        </section>
      ))}
    </div>
  );
}

// --- Signed in -----------------------------------------------------------------

/** The last `clippiboy://add` link already put into the field. */
let takenAddSeq = 0;

function SignedIn() {
  const lists = useFriends((s) => s.lists);
  const presence = useFriends((s) => s.presence);
  const lastSeen = useFriends((s) => s.lastSeen);
  const connected = useFriends((s) => s.connected);
  const me = useFriends((s) => s.me);
  const add = useFriends((s) => s.add);
  const drop = useFriendsExtra((s) => s.drop);
  const favorites = useEngine((s) => s.config.friends.favorites);
  const streamer = useStreamer((s) => s.active);
  const away = useAfter(!connected, AWAY_AFTER_MS);
  const now = useNow();

  const addButton = useRef<HTMLSpanElement>(null);
  const [adding, setAdding] = useState<{ query: string; key: number } | null>(null);

  // "Open ClippiBoy" after "Add <name> as friend" on the website: the code
  // waits in the open popover, sending stays the user's click.
  useEffect(() => {
    if (!add || add.seq <= takenAddSeq) return;
    takenAddSeq = add.seq;
    setAdding({ query: formatCode(add.code), key: add.seq });
  }, [add]);

  const transfers = useShare((s) => s.transfers);
  const ended = useTransferMemory((s) => s.ended);
  const groups = groupFriends(lists.friends, presence, lastSeen, favorites);
  const pending = pendingRows(lists.incoming, lists.outgoing, transfers);

  // Unreachable: one greyed group with the last known state, nothing to send.
  const awayList = away ? awayOrder(lists.friends, lastSeen, drop) : [];
  const awaySeen: Record<string, number> = {};
  for (const friend of awayList) {
    const at = seenWhileAway(friend.id, lastSeen, drop);
    if (at !== undefined) awaySeen[friend.id] = at;
  }

  return (
    <>
      <Header>
        {streamer && (
          <span className="mr-1.5 rounded-pill bg-accent/20 px-2.5 py-[3px] text-xs font-medium text-accent-bright">
            Streamer mode
          </span>
        )}
        <StatusChip me={me} connected={connected} />
        <span ref={addButton}>
          <Button
            variant="primary"
            icon={<IconPlus className="h-4 w-4" />}
            aria-expanded={adding !== null}
            onClick={() => setAdding((open) => (open ? null : { query: "", key: Date.now() }))}
          >
            Add friend
          </Button>
        </span>
      </Header>

      {adding && (
        <AddFriendPopover
          key={adding.key}
          anchor={addButton}
          me={me}
          initialQuery={adding.query}
          onClose={() => setAdding(null)}
        />
      )}

      {away && (
        <Card className="flex items-center gap-4 px-5 py-4">
          <span className="h-[7px] w-[7px] shrink-0 rounded-pill bg-warn shadow-[0_0_0_3px_rgba(245,158,11,0.2)]" />
          <div className="min-w-0 flex-1">
            <p className="text-sm font-semibold">Can't reach the friends server</p>
            <p className="mt-0.5 text-[13px] text-ink-muted">Trying again by itself</p>
          </div>
        </Card>
      )}

      {pending.length > 0 && (
        <section>
          <GroupTitle title="Pending" count={pending.length} />
          <Card className="divide-y divide-line">
            {pending.map((row) =>
              row.kind === "offer" ? (
                <OfferRow key={`offer-${row.transfer.id}`} transfer={row.transfer} presence={presence} />
              ) : (
                <RequestRow key={`${row.kind}-${row.request.id}`} request={row.request} incoming={row.kind === "request"} now={now} />
              ),
            )}
          </Card>
        </section>
      )}

      {lists.friends.length === 0 ? (
        <EmptyCard me={me} streamer={streamer} onAdd={() => setAdding({ query: "", key: Date.now() })} />
      ) : away ? (
        <Group title="Friends" friends={awayList} presence={{}} lastSeen={awaySeen} now={now} transfers={[]} ended={ended} />
      ) : (
        <>
          <Group title="In game" friends={groups.playing} {...{ presence, lastSeen, now, transfers, ended }} />
          <Group title="Online" friends={groups.online} {...{ presence, lastSeen, now, transfers, ended }} />
          <Group title="Offline" friends={groups.offline} collapsible {...{ presence, lastSeen, now, transfers, ended }} />
        </>
      )}
    </>
  );
}

/** Copies and says so on the button for a moment. */
function useCopy(): [string | null, (what: string, text: string) => void] {
  const [copied, setCopied] = useState<string | null>(null);
  const timer = useRef(0);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  const copy = (what: string, text: string) => {
    void navigator.clipboard
      .writeText(text)
      .then(() => {
        setCopied(what);
        window.clearTimeout(timer.current);
        timer.current = window.setTimeout(() => setCopied(null), 1500);
      })
      .catch(() => toast("error", "Could not copy to the clipboard."));
  };
  return [copied, copy];
}

/** No friends yet: one's code and the two ways to bring someone in. */
function EmptyCard({ me, streamer, onAdd }: { me: FriendMe | null; streamer: boolean; onAdd: () => void }) {
  const [copied, copy] = useCopy();
  return (
    <Card className="mx-auto mt-14 flex w-[440px] max-w-full flex-col items-center gap-3.5 px-8 pt-9 pb-8 text-center">
      <span className="grid h-14 w-14 place-items-center rounded-pill bg-accent/15 text-accent-bright">
        <IconFriends className="h-7 w-7" />
      </span>
      <p className="text-lg font-semibold">No friends yet</p>
      {me && (
        <button
          onClick={() => copy("code", formatCode(me.friendCode))}
          title="Copy your friend code"
          className="flex h-9 items-center gap-2 rounded-pill border border-line bg-elevated px-3.5 font-mono text-sm
            tracking-[0.08em] transition-colors hover:bg-hover"
        >
          {streamer ? maskedCode(me.friendCode) : formatCode(me.friendCode)}
          {copied === "code" ? <IconCheck className="h-4 w-4 text-ok" /> : <IconCopy className="h-4 w-4 text-ink-muted" />}
        </button>
      )}
      <div className="flex gap-2">
        <Button
          variant="primary"
          disabled={!me}
          icon={copied === "link" ? <IconCheck className="h-4 w-4" /> : <IconLink className="h-4 w-4" />}
          onClick={() => me && copy("link", inviteLink(me.friendCode))}
        >
          {copied === "link" ? "Copied" : "Copy invite link"}
        </Button>
        <Button icon={<IconPlus className="h-4 w-4" />} onClick={onAdd}>
          Add by name
        </Button>
      </div>
    </Card>
  );
}

// --- Status --------------------------------------------------------------------

const CHOICES: StatusChoice[] = ["online", "busy", "invisible"];

/** The dot in the chip and the menu: the same colours as on the friends. */
function StatusDot({ status }: { status: MyStatus }) {
  return (
    <span
      className={cn(
        "h-[7px] w-[7px] shrink-0 rounded-pill",
        status === "online" && "bg-ok",
        status === "busy" && "bg-live",
        status === "invisible" && "shadow-[inset_0_0_0_1.5px_var(--color-ink-muted)]",
        status === "connecting" && "bg-warn shadow-[0_0_0_3px_rgba(245,158,11,0.2)]",
      )}
    />
  );
}

function StatusChip({ me, connected }: { me: FriendMe | null; connected: boolean }) {
  const { config, patchConfig } = useEngine(useShallow((s) => ({ config: s.config.friends, patchConfig: s.patchConfig })));
  const [open, setOpen] = useState(false);
  const anchor = useRef<HTMLSpanElement>(null);
  const status = myStatus(config, connected);

  return (
    <>
      <span ref={anchor}>
        <button
          aria-haspopup="dialog"
          aria-expanded={open}
          aria-label={`Your status: ${STATUS_LABEL[status]}`}
          onClick={() => setOpen((v) => !v)}
          className="flex h-10 items-center gap-2 rounded-pill border border-line bg-surface pr-3 pl-[5px] text-[13px]
            font-medium transition-colors hover:bg-elevated"
        >
          {me ? <Avatar user={me} size="sm" /> : <span className="h-[22px] w-[22px] rounded-pill bg-elevated" />}
          <StatusDot status={status} />
          {STATUS_LABEL[status]}
          <svg viewBox="0 0 12 12" aria-hidden className="ml-0.5 h-2.5 w-2.5 text-ink-muted" fill="currentColor">
            <path d="M2.5 4.5h7L6 8.5Z" />
          </svg>
        </button>
      </span>
      {open && (
        <Popover anchor={anchor} label="Your status" onClose={() => setOpen(false)} className="w-[280px]">
          <StatusMenu
            current={statusChoice(config)}
            status={config.status}
            onPick={(choice) => {
              setOpen(false);
              void patchConfig({ friends: { ...useEngine.getState().config.friends, ...statusPatch(choice) } });
            }}
            onStatus={(text) => void patchConfig({ friends: { ...useEngine.getState().config.friends, status: text } })}
            onSettings={() => {
              setOpen(false);
              openSettings("friends");
            }}
          />
        </Popover>
      )}
    </>
  );
}

function StatusMenu({
  current,
  status,
  onPick,
  onStatus,
  onSettings,
}: {
  current: StatusChoice;
  status: string;
  onPick: (choice: StatusChoice) => void;
  onStatus: (text: string) => void;
  onSettings: () => void;
}) {
  const [text, setText] = useState(status);
  // Saved on Enter, on leaving the field, and when the menu closes.
  const latest = useRef({ text, status, onStatus });
  latest.current = { text, status, onStatus };
  const save = () => {
    const { text, status, onStatus } = latest.current;
    const clean = text.trim().slice(0, 60);
    if (clean !== status) onStatus(clean);
  };
  useEffect(() => save, []);

  const item = "flex h-9 w-full items-center gap-2.5 rounded-[10px] px-3 text-left text-[13px] font-medium transition-colors";
  return (
    <div className="p-1.5">
      {CHOICES.map((choice) => (
        <button
          key={choice}
          aria-pressed={current === choice}
          onClick={() => onPick(choice)}
          className={cn(item, current === choice ? "bg-white/8" : "hover:bg-hover")}
        >
          <StatusDot status={choice} />
          <span className="flex-1">{STATUS_LABEL[choice]}</span>
          {current === choice && <IconCheck className="h-4 w-4 text-ink-muted" />}
        </button>
      ))}
      <div className="mx-2 my-1.5 border-t border-white/8" />
      <input
        value={text}
        maxLength={60}
        onChange={(e) => setText(e.target.value)}
        onBlur={save}
        onKeyDown={(e) => {
          if (e.key === "Enter") (e.target as HTMLInputElement).blur();
        }}
        placeholder="What are you up to?"
        aria-label="Your status text"
        className="h-[34px] w-full rounded-pill border border-line bg-surface px-3 text-[13px] outline-none
          transition-colors placeholder:text-ink-faint focus:border-line-strong"
      />
      <div className="mx-2 my-1.5 border-t border-white/8" />
      <button onClick={onSettings} className={cn(item, "hover:bg-hover")}>
        <IconSettings className="h-4 w-4 text-ink-muted" />
        Friends settings
      </button>
    </div>
  );
}

// --- Add friend ----------------------------------------------------------------

function AddFriendPopover({
  anchor,
  me,
  initialQuery,
  onClose,
}: {
  anchor: RefObject<HTMLElement | null>;
  me: FriendMe | null;
  initialQuery: string;
  onClose: () => void;
}) {
  const [query, setQuery] = useState(initialQuery);
  const [busy, setBusy] = useState(false);
  /** The outcome and who was already there, so the newcomer can be named. */
  const [result, setResult] = useState<{ outcome: AddOutcome; before: Set<string> } | null>(null);
  const lists = useFriends((s) => s.lists);
  const streamer = useStreamer((s) => s.active);
  const [revealed, setRevealed] = useState(false);
  const [copied, copy] = useCopy();
  const input = useRef<HTMLInputElement>(null);

  useEffect(() => {
    input.current?.focus();
    input.current?.select();
  }, []);

  // The eye shows the code for a few seconds, then streamer mode hides it again.
  useEffect(() => {
    if (!revealed) return;
    const timer = window.setTimeout(() => setRevealed(false), 5000);
    return () => window.clearTimeout(timer);
  }, [revealed]);

  const submit = (event: FormEvent) => {
    event.preventDefault();
    const text = query.trim();
    if (!text || busy) return;
    const { friends, outgoing } = useFriends.getState().lists;
    const before = new Set([...friends, ...outgoing].map((u) => u.id));
    setBusy(true);
    setResult(null);
    friendsApi
      .request(text)
      .then((status) => {
        setQuery("");
        setResult({ outcome: { ok: true, status }, before });
      })
      .catch((err) => setResult({ outcome: { ok: false, error: String(err) }, before }))
      .finally(() => setBusy(false));
  };

  const answer =
    result &&
    addAnswer(
      result.outcome,
      result.outcome.ok
        ? (newcomer(result.before, result.outcome.status === "accepted" ? lists.friends : lists.outgoing)?.displayName ?? null)
        : null,
    );

  const row = "flex h-10 items-center gap-2.5 px-1.5 text-[13px] text-ink/80";
  const iconButton =
    "grid h-7 w-7 shrink-0 place-items-center rounded-pill text-ink transition-colors hover:bg-hover";

  return (
    <Popover anchor={anchor} label="Add friend" onClose={onClose} className="w-[440px] max-w-[calc(100vw-16px)]">
      <div className="p-3">
        <form onSubmit={submit} className="flex gap-2">
          <input
            ref={input}
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setResult(null);
            }}
            placeholder="Discord name or friend code"
            aria-label="Discord name or friend code"
            className="h-10 min-w-0 flex-1 rounded-pill border border-line bg-surface px-4 text-sm outline-none
              transition-colors placeholder:text-ink-faint focus:border-line-strong"
          />
          <Button type="submit" variant="primary" disabled={!query.trim() || busy}>
            Add
          </Button>
        </form>
        {answer && (
          <p
            role="status"
            className={cn(
              "mx-2 mt-2.5 flex items-center gap-1.5 text-xs",
              answer.tone === "ok" ? "text-ok" : answer.tone === "info" ? "text-ink-muted" : "text-live",
            )}
          >
            {answer.tone === "bad" ? <IconClose className="h-3.5 w-3.5 shrink-0" /> : <IconCheck className="h-3.5 w-3.5 shrink-0" />}
            {answer.text}
          </p>
        )}
        {me && (
          <>
            <div className="mx-1 my-2.5 border-t border-white/8" />
            <div className={row}>
              <IconLink className="h-4 w-4 shrink-0 text-ink-muted" />
              Invite link
              <span className="ml-auto truncate text-ink-muted">{streamer ? "Hidden" : shortLink(me.friendCode)}</span>
              <button
                aria-label="Copy invite link"
                title="Copy invite link"
                onClick={() => copy("link", inviteLink(me.friendCode))}
                className={iconButton}
              >
                {copied === "link" ? <IconCheck className="h-[15px] w-[15px] text-ok" /> : <IconCopy className="h-[15px] w-[15px]" />}
              </button>
            </div>
            <div className={row}>
              <IconHash className="h-4 w-4 shrink-0 text-ink-muted" />
              Your code
              <span className="ml-auto font-mono text-ink tracking-[0.04em]">
                {streamer && !revealed ? maskedCode(me.friendCode) : formatCode(me.friendCode)}
              </span>
              {streamer && (
                <button
                  aria-label="Show the code for a moment"
                  title="Show the code for a moment"
                  aria-pressed={revealed}
                  onClick={() => setRevealed((v) => !v)}
                  className={cn(iconButton, !revealed && "text-ink-muted")}
                >
                  <IconEye className="h-[15px] w-[15px]" />
                </button>
              )}
              <button
                aria-label="Copy your friend code"
                title="Copy your friend code"
                onClick={() => copy("code", formatCode(me.friendCode))}
                className={iconButton}
              >
                {copied === "code" ? <IconCheck className="h-[15px] w-[15px] text-ok" /> : <IconCopy className="h-[15px] w-[15px]" />}
              </button>
            </div>
            {!me.allowRequests && (
              <>
                <div className="mx-1 my-2.5 border-t border-white/8" />
                <div className={cn(row, "text-ink-muted")}>
                  <span className="flex-1">You don't accept requests right now</span>
                  <Button size="sm" onClick={() => void friendsApi.setAllowRequests(true).catch(report)}>
                    Turn on
                  </Button>
                </div>
              </>
            )}
          </>
        )}
      </div>
    </Popover>
  );
}

/** A group's small title with its count, like the day groups in Clips. */
function GroupTitle({ title, count, children }: { title: string; count: number; children?: ReactNode }) {
  return (
    <div className="mb-2.5 flex items-center gap-2.5">
      <h2 className="flex items-baseline gap-2.5 text-[15px] font-semibold text-ink/80">
        {title}
        <span className="text-[13px] font-normal text-ink-faint tabular-nums">{count}</span>
      </h2>
      {children}
    </div>
  );
}

/** Round chevron like a settings group's, for the Offline group. */
function Chevron({ open, label, onClick }: { open: boolean; label: string; onClick: () => void }) {
  return (
    <button
      type="button"
      aria-expanded={open}
      aria-label={label}
      title={open ? "Hide offline" : "Show offline"}
      onClick={onClick}
      className="ml-auto grid size-6 shrink-0 place-items-center rounded-pill border border-line bg-elevated
        text-ink-muted transition-colors hover:bg-hover hover:text-ink"
    >
      <svg
        viewBox="0 0 12 12"
        aria-hidden
        className={cn(
          "size-2.5 transition-transform duration-200 ease-[var(--ease-out-soft)] motion-reduce:transition-none",
          open && "rotate-180",
        )}
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <path d="M2.5 4.5 6 8l3.5-3.5" />
      </svg>
    </button>
  );
}

const ROW = "flex min-h-[62px] items-center gap-3.5 px-5 py-2.5";
/** Round icon button in a row. */
const ICON_BUTTON =
  "grid h-[30px] w-[30px] shrink-0 place-items-center rounded-pill transition-colors hover:bg-hover hover:text-ink";

function RequestRow({ request, incoming, now }: { request: FriendPending; incoming: boolean; now: number }) {
  const [busy, setBusy] = useState(false);
  const run = (action: Promise<void>) => {
    setBusy(true);
    action.catch(report).finally(() => setBusy(false));
  };
  return (
    <div className={ROW}>
      <span className={cn("contents", !incoming && "[&>*]:opacity-55")}>
        <Avatar user={request} />
        <div className="min-w-0 flex-1">
          <p className="truncate text-sm font-medium">{request.displayName}</p>
          <p className="mt-0.5 truncate text-xs text-ink-muted">
            {incoming ? "Friend request" : "Request sent"} · {ago(request.since, now)}
          </p>
        </div>
      </span>
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
        <Button
          size="sm"
          variant="ghost"
          disabled={busy}
          className="opacity-70 hover:opacity-100"
          onClick={() => run(friendsApi.remove(request.id))}
        >
          Withdraw
        </Button>
      )}
    </div>
  );
}

/** A clip someone wants to send: who, what, and the answer. */
function OfferRow({ transfer: t, presence }: { transfer: Transfer; presence: Record<string, FriendPresence> }) {
  const friend = useFriends((s) => s.lists.friends.find((f) => f.id === t.friendId));
  const [busy, setBusy] = useState(false);
  const run = (action: Promise<void>) => {
    setBusy(true);
    action.catch(report).finally(() => setBusy(false));
  };
  const user = friend ?? { id: t.friendId, username: t.friendName, displayName: t.friendName, avatar: null };
  const what = noun(t);
  return (
    <div className={ROW}>
      <Avatar user={user} status={friendStatus(presence[t.friendId])} />
      <div className="min-w-0 flex-1">
        <p className="truncate text-sm font-medium">{user.displayName}</p>
        <p className="mt-0.5 truncate text-xs text-ink-muted">
          Wants to send you a {what} · {t.name} · {megabytes(t.size)}
        </p>
      </div>
      {/* The offer carries no picture — a still in the clip's colours of the
          gallery stands in for one. */}
      <span
        aria-hidden
        className="grid h-9 w-16 shrink-0 place-items-center rounded-lg bg-[linear-gradient(135deg,#34183a_0%,#160b1c_50%,#000_100%)]
          text-[10px] font-semibold text-white/70"
      >
        {!t.screenshot && t.durationMs > 0 ? formatDuration(t.durationMs) : null}
      </span>
      <div className="flex gap-2">
        <Button size="sm" variant="ghost" disabled={busy} onClick={() => run(shareApi.decline(t.id))}>
          Decline
        </Button>
        <Button size="sm" variant="primary" disabled={busy} onClick={() => run(shareApi.accept(t.id))}>
          Accept
        </Button>
      </div>
    </div>
  );
}

function Group({
  title,
  friends,
  presence,
  lastSeen,
  now,
  transfers,
  ended,
  collapsible = false,
}: {
  title: string;
  friends: FriendUser[];
  presence: Record<string, FriendPresence>;
  lastSeen: Record<string, number>;
  now: number;
  transfers: Transfer[];
  ended: Record<string, Ended>;
  /** Folds away like the console's "N offline"; the choice is remembered. */
  collapsible?: boolean;
}) {
  const storageKey = `friends.${title.toLowerCase()}`;
  const [open, setOpen] = useState(() => !collapsible || readOpen(storageKey, true));
  if (friends.length === 0) return null;
  const toggle = () => {
    setOpen(!open);
    writeOpen(storageKey, !open);
  };
  return (
    <section>
      <GroupTitle title={title} count={friends.length}>
        {collapsible && <Chevron open={open} label={title} onClick={toggle} />}
      </GroupTitle>
      {open && (
        <Card className="divide-y divide-line">
          {friends.map((friend) => (
            <FriendRow
              key={friend.id}
              friend={friend}
              presence={presence[friend.id] ?? null}
              lastSeen={lastSeen[friend.id]}
              now={now}
              transfer={rowTransfer(friend.id, transfers, ended)}
            />
          ))}
        </Card>
      )}
    </section>
  );
}

function FriendRow({
  friend,
  presence,
  lastSeen,
  now,
  transfer,
}: {
  friend: FriendUser;
  presence: FriendPresence | null;
  lastSeen: number | undefined;
  now: number;
  transfer: RowTransfer | null;
}) {
  const menu = useMenu();
  const { config, patchConfig } = useEngine(useShallow((s) => ({ config: s.config.friends, patchConfig: s.patchConfig })));
  const favorite = config.favorites.includes(friend.id);
  const status: Status = friendStatus(presence);
  const duration = presence?.game ? playingFor(presence.since, now) : null;
  const [removing, setRemoving] = useState(false);

  const toggleFavorite = () =>
    void patchConfig({
      friends: {
        ...config,
        favorites: favorite ? config.favorites.filter((id) => id !== friend.id) : [...config.favorites, friend.id],
      },
    });

  const chooseFor = useShare((s) => s.chooseFor);
  const reachable = canReceive(presence);

  const openMenu = (event: MouseEvent) =>
    menu.open(event, [
      ...(reachable
        ? [
            {
              kind: "item" as const,
              label: "Send a clip…",
              icon: <IconSend className="h-4 w-4" />,
              onSelect: () => chooseFor(friend),
            },
            { kind: "separator" as const },
          ]
        : []),
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
        // Asks once, in the row, like Delete on a clip.
        onSelect: () => setRemoving(true),
      },
      {
        kind: "item",
        label: "Block",
        danger: true,
        onSelect: () => void friendsApi.block(friend.id).catch(report),
      },
    ]);

  const running = transfer?.kind === "sending" || transfer?.kind === "receiving";
  const failed = transfer?.kind === "error";
  const sendButton = reachable && !running && !failed && (
    <button
      aria-label={`Send a clip to ${friend.displayName}`}
      title="Send a clip"
      onClick={() => chooseFor(friend)}
      className={cn(ICON_BUTTON, "bg-elevated text-ink")}
    >
      <IconSend className="h-[15px] w-[15px]" />
    </button>
  );
  const moreButton = !running && !failed && (
    <button
      aria-label={`More for ${friend.displayName}`}
      onClick={openMenu}
      className={cn(ICON_BUTTON, "text-ink-muted", status === "offline" && "opacity-55")}
    >
      <svg viewBox="0 0 24 24" className="h-4 w-4" fill="currentColor">
        <circle cx="5" cy="12" r="1.8" />
        <circle cx="12" cy="12" r="1.8" />
        <circle cx="19" cy="12" r="1.8" />
      </svg>
    </button>
  );

  return (
    <div onContextMenu={openMenu} className={ROW}>
      <span className={cn("contents", status === "offline" && "[&>*]:opacity-55")}>
        <Avatar user={friend} status={status} />
        <div className="min-w-0 flex-1">
          <p className="flex min-w-0 items-center gap-1.5 text-sm font-medium">
            <span className="truncate">{friend.displayName}</span>
            {favorite && <IconHeart filled className="h-3.5 w-3.5 shrink-0 text-accent-bright" />}
            {presence?.status && (
              <span className="truncate text-xs font-normal text-ink-muted">— {presence.status}</span>
            )}
          </p>
          {transfer ? (
            <TransferLine transfer={transfer} now={now} />
          ) : presence?.game ? (
            <p className="mt-0.5 flex min-w-0 items-center gap-1.5 text-xs text-accent-bright">
              <IconGamepad className="h-3.5 w-3.5 shrink-0" />
              <span className="truncate">{presence.game}</span>
              {duration && <span className="shrink-0 text-ink-muted">· {duration}</span>}
              {presence.busy && <span className="shrink-0 text-live">· Busy</span>}
            </p>
          ) : (
            <p className={cn("mt-0.5 truncate text-xs", status === "busy" ? "text-live" : "text-ink-muted")}>
              {status === "busy" ? "Busy" : status === "online" ? "Online" : lastSeenText(lastSeen, now)}
            </p>
          )}
        </div>
      </span>
      {removing ? (
        <ConfirmDelete
          question={`Remove ${friend.displayName}?`}
          origin="right"
          confirmLabel="Remove friend"
          confirmTitle="Remove — you would have to add each other again"
          cancelLabel="Keep"
          cancelTitle="Keep — Escape does the same"
          onConfirm={() => {
            setRemoving(false);
            void friendsApi.remove(friend.id).catch(report);
          }}
          onCancel={() => setRemoving(false)}
        />
      ) : (
        <>
          {transfer && <TransferActions transfer={transfer} reachable={reachable} />}
          {sendButton}
          {moreButton}
        </>
      )}
    </div>
  );
}

/** The line under the name while a clip goes to or comes from that friend. */
function TransferLine({ transfer: t, now }: { transfer: RowTransfer; now: number }) {
  const line = "mt-0.5 flex min-w-0 items-center gap-1.5 text-xs";
  if (t.kind === "sending" || t.kind === "receiving")
    return (
      <p className={cn(line, "text-accent-bright")}>
        {t.kind === "sending" ? <IconSend className="h-3.5 w-3.5 shrink-0" /> : <IconClips className="h-3.5 w-3.5 shrink-0" />}
        <span className="truncate">{t.text}</span>
      </p>
    );
  if (t.kind === "error")
    return (
      <p className={cn(line, "text-live")}>
        <span className="truncate">{t.text}</span>
      </p>
    );
  return (
    <p className={cn(line, t.tone === "ok" ? "text-ok" : "text-ink-muted")}>
      {t.tone === "ok" && <IconCheck className="h-3.5 w-3.5 shrink-0" />}
      <span className="truncate">
        {t.text} · {ago(t.at, now)}
      </span>
    </p>
  );
}

/** On the right of the row: the bar and its X, Retry and its X, or Open. */
function TransferActions({ transfer: t, reachable }: { transfer: RowTransfer; reachable: boolean }) {
  const closeButton = (label: string, onClick: () => void) => (
    <button aria-label={label} title={label} onClick={onClick} className={cn(ICON_BUTTON, "bg-elevated text-ink-muted")}>
      <IconClose className="h-3.5 w-3.5" />
    </button>
  );
  if (t.kind === "sending" || t.kind === "receiving")
    return (
      <>
        {t.share !== null && (
          <span className="flex w-[200px] shrink-0 items-center gap-2.5 font-mono text-xs text-ink-muted tabular-nums">
            <ProgressBar share={t.share} className="flex-1" />
            {Math.floor(t.share * 100)} %
          </span>
        )}
        {closeButton(t.ids.length > 1 ? "Cancel all" : "Cancel", () => {
          for (const id of t.ids) void shareApi.cancel(id);
        })}
      </>
    );
  if (t.kind === "error")
    return (
      <>
        {t.retry.length > 0 && (
          <Button
            size="sm"
            icon={<IconRetry className="h-3.5 w-3.5" />}
            disabled={!reachable}
            title={reachable ? undefined : "They aren't online right now"}
            onClick={() => retryTransfers(t.retry)}
          >
            Retry
          </Button>
        )}
        {closeButton("Dismiss", () => {
          for (const id of t.ids) void shareApi.dismiss(id);
        })}
      </>
    );
  return t.open ? (
    <Button size="sm" variant="ghost" onClick={() => openClip(t.open!)}>
      Open
    </Button>
  ) : null;
}

// --- Bits ----------------------------------------------------------------------

type Status = FriendStatus | "invisible";

export function Avatar({ user, status, size = "md" }: { user: FriendUser; status?: Status; size?: "sm" | "md" | "lg" }) {
  const [broken, setBroken] = useState(false);
  const box = size === "lg" ? "h-14 w-14" : size === "sm" ? "h-[22px] w-[22px] text-[11px]" : "h-10 w-10";
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

/** True once `active` has held for `ms` without a break. */
function useAfter(active: boolean, ms: number): boolean {
  const [late, setLate] = useState(false);
  useEffect(() => {
    setLate(false);
    if (!active) return;
    const timer = window.setTimeout(() => setLate(true), ms);
    return () => window.clearTimeout(timer);
  }, [active, ms]);
  return active && late;
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
