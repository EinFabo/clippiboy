import { DurableObject } from "cloudflare:workers";
import { friendIds } from "./db";

/** What friends see. `null` means offline — or invisible, which looks the same. */
export interface Presence {
  game: string | null;
  /** When the game started, in ms since the epoch. */
  since: number | null;
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

/**
 * One per user, named by the user id. Holds that user's app connections and
 * forwards their presence to the hubs of their friends, which forward it to
 * their own connections. Hibernates between messages.
 */
export class Hub extends DurableObject<Env> {
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
      presence: { game: null, since: null },
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
    let data: { t?: string; invisible?: unknown; game?: unknown; since?: unknown };
    try {
      data = JSON.parse(message);
    } catch {
      return;
    }
    if (data.t !== "hello" && data.t !== "presence") return;
    const attachment = ws.deserializeAttachment() as Attachment;
    attachment.ready = true;
    attachment.invisible = data.invisible === true;
    attachment.presence = sanitize(data.game, data.since);
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

  /** This user's presence as friends should see it. */
  async presence(): Promise<Presence | null> {
    return (await this.ctx.storage.get<Presence | null>(PUBLISHED)) ?? null;
  }

  /** A friend's presence changed. */
  async friendPresence(friend: string, presence: Presence | null): Promise<void> {
    this.broadcast({ t: "presence", id: friend, presence });
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

  /** Tells the friends — but only when something they can see has changed. */
  private async publish(user: string, exclude?: WebSocket): Promise<void> {
    const next = this.current(exclude);
    const previous = (await this.ctx.storage.get<Presence | null>(PUBLISHED)) ?? null;
    if (JSON.stringify(previous) === JSON.stringify(next)) return;
    await this.ctx.storage.put(PUBLISHED, next);
    const friends = await friendIds(this.env.DB, user);
    await Promise.all(
      friends.map((id) =>
        this.env.HUB.getByName(id)
          .friendPresence(user, next)
          .catch((err) => console.error("presence to", id, err)),
      ),
    );
  }

  private async snapshot(user: string): Promise<string> {
    const friends = await friendIds(this.env.DB, user);
    const presences = await Promise.all(
      friends.map((id) =>
        this.env.HUB.getByName(id)
          .presence()
          .catch(() => null),
      ),
    );
    return JSON.stringify({
      t: "snapshot",
      friends: Object.fromEntries(friends.map((id, i) => [id, presences[i]])),
    });
  }

  private async sendSnapshot(ws: WebSocket, user: string): Promise<void> {
    ws.send(await this.snapshot(user));
  }
}

function sanitize(game: unknown, since: unknown): Presence {
  const name = typeof game === "string" ? game.trim().slice(0, 80) : "";
  if (!name) return { game: null, since: null };
  const now = Date.now();
  const valid = typeof since === "number" && since > now - 7 * 24 * 3600_000 && since <= now + 60_000;
  return { game: name, since: valid ? since : now };
}
