import { DurableObject } from "cloudflare:workers";
import { areFriends, friendIds } from "./db";
import {
  beyondCap,
  HELLO,
  PRESENCE,
  RAW_MAX,
  RELAY,
  SOCKET_CAP,
  mayReach,
  remember,
  spend,
  type Bucket,
  type Rule,
  type Contacts,
} from "./hubrules";

/** What friends see. `null` means offline — or invisible, which looks the same. */
export interface Presence {
  game: string | null;
  /** When the game started, in ms since the epoch. */
  since: number | null;
  /** A line of the user's own, or null. */
  status: string | null;
  /** Do not disturb: online, but not for requests. */
  busy: boolean;
}

/** A friend as a snapshot shows them: online or not, and when last seen. */
export interface Card {
  presence: Presence | null;
  lastSeen: number | null;
}

interface Attachment {
  user: string;
  /** False until the app has said hello; such a socket does not count yet. */
  ready: boolean;
  invisible: boolean;
  presence: Presence;
  /** Last message from the app, as a fallback to the ping timestamp. */
  at: number;
  /** When the socket was accepted; the oldest go first when there are too many. */
  opened: number;
}

/** Pings come every 30 s; three missed ones and the socket is gone. */
const STALE_MS = 100_000;
/** How often a hub with open sockets wakes to look for dead ones. Every
    minute cost an alarm per online user around the clock; STALE_MS still
    decides what counts as dead. */
const SWEEP_MS = 300_000;
const PUBLISHED = "published";
const LAST_SEEN = "lastSeen";
/** Friends an invisible user wrote to lately; kept in storage, memory is lost on hibernation. */
const CONTACTS = "contacts";
/** Presence and hello budgets, so they survive the hub going to sleep. */
const BUCKETS = "buckets";
/** A relayed message is a handshake, never a file: a few KB are plenty. */
const RELAY_MAX = 4096;

const NO_PRESENCE: Presence = { game: null, since: null, status: null, busy: false };

/**
 * One per user, named by the user id. Holds that user's app connections and
 * forwards their presence to the hubs of their friends, which forward it to
 * their own connections. Hibernates between messages.
 */
export class Hub extends DurableObject<Env> {
  /** One budget per user across all connections. Relay lives in memory only (it refills within the
      ~10 s before hibernation anyway); presence and hello are kept in storage, see `take`. */
  private buckets = new Map<"relay" | "presence" | "hello", Bucket>();

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    // Answered by the runtime without waking the object.
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
  }

  /** The WebSocket upgrade; the worker has checked the token and names the user. */
  async fetch(request: Request): Promise<Response> {
    const user = request.headers.get("X-User");
    if (!user || request.headers.get("Upgrade") !== "websocket") {
      return new Response("Expected a WebSocket", { status: 426 });
    }
    const [client, server] = Object.values(new WebSocketPair());
    this.ctx.acceptWebSocket(server);
    const attachment: Attachment = {
      user,
      ready: false,
      invisible: false,
      presence: NO_PRESENCE,
      at: Date.now(),
      opened: Date.now(),
    };
    server.serializeAttachment(attachment);
    // The newest connections stay; the app dials again after a 4002.
    const sockets = this.open().map((ws) => ({ ws, opened: (ws.deserializeAttachment() as Attachment).opened }));
    for (const { ws } of beyondCap(sockets, SOCKET_CAP)) {
      try {
        ws.close(4002, "too many connections");
      } catch {
        // Already closed.
      }
    }
    if ((await this.ctx.storage.getAlarm()) === null) {
      await this.ctx.storage.setAlarm(Date.now() + SWEEP_MS);
    }
    return new Response(null, { status: 101, webSocket: client });
  }

  async webSocketMessage(ws: WebSocket, message: string | ArrayBuffer): Promise<void> {
    if (typeof message !== "string" || message.length > RAW_MAX) return;
    let data: PresenceInput & { t?: string; invisible?: unknown; to?: unknown; body?: unknown };
    try {
      data = JSON.parse(message);
    } catch {
      return;
    }
    if (data.t === "relay") return this.relayFrom(ws, data.to, data.body);
    if (data.t !== "hello" && data.t !== "presence") return;
    if (data.t === "hello") {
      if (!(await this.take("hello", HELLO, Date.now())).ok) return ws.close(4003, "slow down");
    }
    const attachment = ws.deserializeAttachment() as Attachment;
    attachment.ready = true;
    attachment.invisible = data.invisible === true;
    attachment.presence = sanitize(data);
    attachment.at = Date.now();
    ws.serializeAttachment(attachment);
    if (data.t === "hello") await this.sendSnapshot(ws, attachment.user);
    await this.publishSoon(attachment.user);
  }

  async webSocketClose(ws: WebSocket, code: number, reason: string): Promise<void> {
    const { user } = ws.deserializeAttachment() as Attachment;
    try {
      ws.close(code, reason);
    } catch {
      // Already closed.
    }
    await this.publish(user, ws);
  }

  async webSocketError(ws: WebSocket): Promise<void> {
    const { user } = ws.deserializeAttachment() as Attachment;
    await this.publish(user, ws);
  }

  /** Closes connections that stopped pinging — a crash or a dead network never says goodbye. */
  async alarm(): Promise<void> {
    const now = Date.now();
    let user: string | null = null;
    for (const ws of this.ctx.getWebSockets()) {
      const attachment = ws.deserializeAttachment() as Attachment;
      user = attachment.user;
      const pinged = this.ctx.getWebSocketAutoResponseTimestamp(ws)?.getTime() ?? 0;
      if (now - Math.max(pinged, attachment.at) > STALE_MS) {
        try {
          ws.close(4000, "stale");
        } catch {
          // Already closed.
        }
      }
    }
    if (user) await this.publish(user);
    if (this.open().length > 0) await this.ctx.storage.setAlarm(now + SWEEP_MS);
  }

  // --- Called by the worker and by other hubs ---------------------------------

  /** This user as friends should see them. */
  async card(): Promise<Card> {
    const stored = await this.ctx.storage.get<Presence | null | number>([PUBLISHED, LAST_SEEN]);
    return {
      presence: (stored.get(PUBLISHED) as Presence | null | undefined) ?? null,
      lastSeen: (stored.get(LAST_SEEN) as number | undefined) ?? null,
    };
  }

  /** A friend's presence changed. */
  async friendPresence(friend: string, presence: Presence | null, lastSeen: number | null): Promise<void> {
    this.broadcast({ t: "presence", id: friend, presence, lastSeen });
  }

  /**
   * A friend hands this user's apps a message — the handshake for sending a
   * clip. Nothing is kept. False when no app of this user is there to take it,
   * or the user is invisible and has not written to this friend lately.
   */
  async relay(from: string, body: unknown): Promise<boolean> {
    // While invisible, only friends the user wrote to lately get through: the
    // replies to their own clips. Everyone else sees "offline", as presence does.
    if (this.current() === null && !mayReach(await this.ctx.storage.get<Contacts>(CONTACTS), from, Date.now())) {
      return false;
    }
    const sockets = this.open().filter((ws) => (ws.deserializeAttachment() as Attachment).ready);
    const text = JSON.stringify({ t: "relay", from, body });
    for (const ws of sockets) ws.send(text);
    return sockets.length > 0;
  }

  /** A request, an acceptance, a removal or a block touched this user. */
  async friendsChanged(): Promise<void> {
    const sockets = this.open().filter((ws) => (ws.deserializeAttachment() as Attachment).ready);
    if (sockets.length === 0) return;
    const { user } = sockets[0].deserializeAttachment() as Attachment;
    const snapshot = await this.snapshot(user);
    for (const ws of sockets) {
      ws.send(JSON.stringify({ t: "friends" }));
      ws.send(snapshot);
    }
  }

  /**
   * The account is gone or suspended: drop every connection and forget the
   * presence. Friends hear it went offline first — a suspended account keeps
   * its friends, and once the presence is forgotten the closing sockets have
   * nothing left to compare against and tell nobody.
   */
  async closeAll(reason = "account deleted", user?: string): Promise<void> {
    for (const ws of this.ctx.getWebSockets()) {
      try {
        ws.close(4001, reason);
      } catch {
        // Already closed.
      }
    }
    if (user) await this.publish(user);
    this.buckets.clear();
    await this.ctx.storage.deleteAll();
  }

  // --- Internals ------------------------------------------------------------

  private open(exclude?: WebSocket): WebSocket[] {
    return this.ctx
      .getWebSockets()
      .filter((ws) => ws !== exclude && ws.readyState === WebSocket.OPEN);
  }

  private broadcast(message: unknown): void {
    const text = JSON.stringify(message);
    for (const ws of this.open()) {
      if ((ws.deserializeAttachment() as Attachment).ready) ws.send(text);
    }
  }

  /** The newest word from any of the user's connections decides. */
  private current(exclude?: WebSocket): Presence | null {
    let latest: Attachment | null = null;
    for (const ws of this.open(exclude)) {
      const attachment = ws.deserializeAttachment() as Attachment;
      if (attachment.ready && (!latest || attachment.at > latest.at)) latest = attachment;
    }
    if (!latest || latest.invisible) return null;
    return latest.presence;
  }

  /**
   * Tells the friends — but only when something they can see has changed.
   * Going offline (or invisible, which must look the same) stamps "last seen".
   */
  private async publish(user: string, exclude?: WebSocket): Promise<void> {
    const next = this.current(exclude);
    const { presence: previous, lastSeen: seenBefore } = await this.card();
    if (JSON.stringify(previous) === JSON.stringify(next)) return;
    let lastSeen = seenBefore;
    if (previous && !next) {
      lastSeen = Date.now();
      await this.ctx.storage.put({ [PUBLISHED]: next, [LAST_SEEN]: lastSeen });
    } else {
      await this.ctx.storage.put(PUBLISHED, next);
    }
    const friends = await friendIds(this.env.DB, user);
    await Promise.all(
      friends.map((id) =>
        this.env.HUB.getByName(id)
          .friendPresence(user, next, lastSeen)
          .catch((err) => console.error("presence to", id, err)),
      ),
    );
  }

  /**
   * Spends from the presence or hello budget. These are slower than the ~10 s a
   * hub stays awake, so their state is kept in storage and read again after a wake.
   */
  private async take(name: "presence" | "hello", rule: Rule, now: number): Promise<{ ok: boolean; nextAt: number }> {
    let bucket = this.buckets.get(name);
    if (!bucket) bucket = (await this.ctx.storage.get<Record<string, Bucket>>(BUCKETS))?.[name];
    const result = spend(bucket, rule, now);
    this.buckets.set(name, result.bucket);
    const stored = (await this.ctx.storage.get<Record<string, Bucket>>(BUCKETS)) ?? {};
    await this.ctx.storage.put(BUCKETS, { ...stored, [name]: result.bucket });
    return result;
  }

  /**
   * Publishes at once while the presence budget lasts; after that, the alarm
   * publishes whatever is current when the next token is there, so a burst
   * always ends on its last change.
   */
  private async publishSoon(user: string): Promise<void> {
    const now = Date.now();
    const budget = await this.take("presence", PRESENCE, now);
    if (budget.ok) return this.publish(user);
    const due = await this.ctx.storage.getAlarm();
    if (due === null || due > budget.nextAt) await this.ctx.storage.setAlarm(budget.nextAt);
  }

  /** Passes a message on to a friend's apps, and says so when nobody took it. */
  private async relayFrom(ws: WebSocket, to: unknown, body: unknown): Promise<void> {
    const attachment = ws.deserializeAttachment() as Attachment;
    if (!attachment.ready || typeof to !== "string" || body === undefined) return;
    const ref = typeof body === "object" && body !== null && "id" in body ? (body as { id: unknown }).id : null;
    const fail = (reason: string) => ws.send(JSON.stringify({ t: "relayFailed", to, ref, reason }));

    const now = Date.now();
    const budget = spend(this.buckets.get("relay"), RELAY, now);
    this.buckets.set("relay", budget.bucket);
    if (!budget.ok) return fail("slow down");
    if (JSON.stringify(body).length > RELAY_MAX) return fail("too big");
    // Blocking ends a friendship, so a friend is never someone who blocked.
    if (!(await areFriends(this.env.DB, attachment.user, to))) return fail("not a friend");
    // A ready socket just sent this, so no current presence means invisible.
    if (this.current() === null) {
      await this.ctx.storage.put(CONTACTS, remember((await this.ctx.storage.get<Contacts>(CONTACTS)) ?? {}, to, now));
    }
    const delivered = await this.env.HUB.getByName(to)
      .relay(attachment.user, body)
      .catch((err) => {
        console.error("relay to", to, err);
        return false;
      });
    if (!delivered) fail("offline");
  }

  private async snapshot(user: string): Promise<string> {
    const friends = await friendIds(this.env.DB, user);
    const cards = await Promise.all(
      friends.map((id) =>
        this.env.HUB.getByName(id)
          .card()
          .catch((): Card => ({ presence: null, lastSeen: null })),
      ),
    );
    // `friends` keeps its old shape for apps from before "last seen".
    return JSON.stringify({
      t: "snapshot",
      friends: Object.fromEntries(friends.map((id, i) => [id, cards[i].presence])),
      lastSeen: Object.fromEntries(friends.map((id, i) => [id, cards[i].lastSeen])),
    });
  }

  private async sendSnapshot(ws: WebSocket, user: string): Promise<void> {
    ws.send(await this.snapshot(user));
  }
}

interface PresenceInput {
  game?: unknown;
  since?: unknown;
  status?: unknown;
  busy?: unknown;
}

function sanitize(data: PresenceInput): Presence {
  const text = typeof data.status === "string" ? data.status.replace(/\s+/g, " ").trim() : "";
  const status = [...text].slice(0, 60).join("") || null;
  const busy = data.busy === true;
  const name = typeof data.game === "string" ? data.game.trim().slice(0, 80) : "";
  if (!name) return { game: null, since: null, status, busy };
  const now = Date.now();
  const since = data.since;
  const valid = typeof since === "number" && since > now - 7 * 24 * 3600_000 && since <= now + 60_000;
  return { game: name, since: valid ? since : now, status, busy };
}
