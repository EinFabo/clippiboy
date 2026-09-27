import { DurableObject } from "cloudflare:workers";
import { areFriends, friendIds } from "./db";

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
}

/** Pings come every 30 s; three missed ones and the socket is gone. */
const STALE_MS = 100_000;
const SWEEP_MS = 60_000;
const PUBLISHED = "published";
const LAST_SEEN = "lastSeen";
/** A relayed message is a handshake, never a file: a few KB are plenty. */
const RELAY_MAX = 4096;
/** Per connection, per window — a stuck loop in an app must not flood a friend. */
const RELAY_BURST = 30;
const RELAY_WINDOW_MS = 10_000;

const NO_PRESENCE: Presence = { game: null, since: null, status: null, busy: false };

/**
 * One per user, named by the user id. Holds that user's app connections and
 * forwards their presence to the hubs of their friends, which forward it to
 * their own connections. Hibernates between messages.
 */
export class Hub extends DurableObject<Env> {
  /** Relay budget per connection; lost on hibernation, which is fine. */
  private relayed = new Map<WebSocket, { start: number; count: number }>();

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
    };
    server.serializeAttachment(attachment);
    if ((await this.ctx.storage.getAlarm()) === null) {
      await this.ctx.storage.setAlarm(Date.now() + SWEEP_MS);
    }
    return new Response(null, { status: 101, webSocket: client });
  }

  async webSocketMessage(ws: WebSocket, message: string | ArrayBuffer): Promise<void> {
    if (typeof message !== "string") return;
    let data: PresenceInput & { t?: string; invisible?: unknown; to?: unknown; body?: unknown };
    try {
      data = JSON.parse(message);
    } catch {
      return;
    }
    if (data.t === "relay") return this.relayFrom(ws, data.to, data.body);
    if (data.t !== "hello" && data.t !== "presence") return;
    const attachment = ws.deserializeAttachment() as Attachment;
    attachment.ready = true;
    attachment.invisible = data.invisible === true;
    attachment.presence = sanitize(data);
    attachment.at = Date.now();
    ws.serializeAttachment(attachment);
    if (data.t === "hello") await this.sendSnapshot(ws, attachment.user);
    await this.publish(attachment.user);
  }

  async webSocketClose(ws: WebSocket, code: number, reason: string): Promise<void> {
    const { user } = ws.deserializeAttachment() as Attachment;
    try {
      ws.close(code, reason);
    } catch {
      // Already closed.
    }
    this.relayed.delete(ws);
    await this.publish(user, ws);
  }

  async webSocketError(ws: WebSocket): Promise<void> {
    const { user } = ws.deserializeAttachment() as Attachment;
    this.relayed.delete(ws);
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
   * clip. Nothing is kept. False when no app of this user is there to take it.
   */
  async relay(from: string, body: unknown): Promise<boolean> {
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

  /** The account is gone: drop every connection and forget the presence. */
  async closeAll(): Promise<void> {
    for (const ws of this.ctx.getWebSockets()) {
      try {
        ws.close(4001, "account deleted");
      } catch {
        // Already closed.
      }
    }
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

  /** Passes a message on to a friend's apps, and says so when nobody took it. */
  private async relayFrom(ws: WebSocket, to: unknown, body: unknown): Promise<void> {
    const attachment = ws.deserializeAttachment() as Attachment;
    if (!attachment.ready || typeof to !== "string" || body === undefined) return;
    const ref = typeof body === "object" && body !== null && "id" in body ? (body as { id: unknown }).id : null;
    const fail = (reason: string) => ws.send(JSON.stringify({ t: "relayFailed", to, ref, reason }));

    const now = Date.now();
    const budget = this.relayed.get(ws);
    if (!budget || now - budget.start > RELAY_WINDOW_MS) {
      this.relayed.set(ws, { start: now, count: 1 });
    } else if (++budget.count > RELAY_BURST) {
      return fail("slow down");
    }
    if (JSON.stringify(body).length > RELAY_MAX) return fail("too big");
    // Blocking ends a friendship, so a friend is never someone who blocked.
    if (!(await areFriends(this.env.DB, attachment.user, to))) return fail("not a friend");
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
