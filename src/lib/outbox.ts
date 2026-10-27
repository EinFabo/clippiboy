import { later, takes, type Revised } from "./revision.ts";

/**
 * One write of the settings from a window.
 *
 * `change` is what it does — shown at once, and put onto the core's latest
 * answer when it is sent. `send` writes `next` (that change on `base`) and
 * resolves with the core's answer, or null when there was nothing left to
 * send. `failed` hears a write the bridge refused and says whether the
 * settings should be read again.
 */
export interface Write<T> {
  change: (config: T) => T;
  send: (next: T, base: T) => Promise<T | null>;
  /** Writes with the same key merge while they wait: the later on top. */
  key?: string;
  failed?: (err: unknown) => boolean;
}

/** `write` put behind the queue — merged into the last one if it has its key. */
export function enqueue<T>(queue: readonly Write<T>[], write: Write<T>): Write<T>[] {
  const last = queue[queue.length - 1];
  if (!last || write.key === undefined || last.key !== write.key) return [...queue, write];
  const merged: Write<T> = {
    ...write,
    change: (config) => write.change(last.change(config)),
  };
  return [...queue.slice(0, -1), merged];
}

/** `config` with every queued write done to it, in order. */
export function onTop<T>(config: T, queue: readonly Write<T>[]): T {
  return queue.reduce((acc, write) => write.change(acc), config);
}

export interface OutboxIo<T> {
  /** The settings shown in the window; null until they were read. */
  shown: () => T | null;
  show: (config: T) => void;
  /** The settings as the core holds them now. */
  read: () => Promise<T>;
}

/**
 * A window's settings writes, one at a time.
 *
 * Sent without waiting, every write in one round trip carried the same
 * revision, and the core merged the later ones against it: a switch flipped
 * on and back off, or a slider dragged back to where it started, came back
 * as "unchanged" and the in-between value stayed. Here a write goes out only
 * once the answer to the one before is in, built on that answer — so it
 * carries a revision that already holds this window's earlier writes. What
 * the user does meanwhile is shown at once and waits in the queue, patches
 * of the same kind merged into one write.
 *
 * Settings the core hands over unasked while a write is out are held: taken
 * mid-write, they would show the setting just changed rolled back. Once the
 * queue is empty, the later revision wins — unless the last write was not
 * answered, then the settings are read again rather than trusting a held
 * copy that may predate it.
 */
export class Outbox<T extends Revised> {
  private readonly io: OutboxIo<T>;
  private queue: Write<T>[] = [];
  private readonly waiting = new Map<Write<T>, (() => void)[]>();
  /** The core's latest answer; what the next write is built on. */
  private base: T | null = null;
  private held: T | null = null;
  private lastTaken = true;
  private running = false;

  constructor(io: OutboxIo<T>) {
    this.io = io;
  }

  /** A write is out or waiting. */
  get busy(): boolean {
    return this.running;
  }

  /**
   * Show `write` at once and send it when its turn comes. Resolves once it is
   * answered (or failed). Before the settings were read there is nothing to
   * build on, and nothing is written.
   */
  push(write: Write<T>): Promise<void> {
    const shown = this.io.shown();
    if (!shown) return Promise.resolve();
    if (!this.running) this.base = shown;
    this.io.show(write.change(shown));

    const before = this.queue[this.queue.length - 1];
    this.queue = enqueue(this.queue, write);
    const after = this.queue[this.queue.length - 1];
    const done = new Promise<void>((resolve) => {
      // Merged into the one before: its callers wait for the merged write.
      const earlier = after !== write && before ? (this.waiting.get(before) ?? []) : [];
      if (before && after !== write) this.waiting.delete(before);
      this.waiting.set(after, [...earlier, resolve]);
    });
    if (!this.running) void this.run();
    return done;
  }

  /** Settings from the core that no write of this window asked for. */
  offer(config: T) {
    this.hand(config, false);
  }

  /** Settings just read: they also stand in for their own revision. */
  read(config: T) {
    this.hand(config, true);
  }

  private hand(config: T, orSame: boolean) {
    if (this.running) {
      this.held = later(this.held, config);
      return;
    }
    const shown = this.io.shown();
    if (!shown || takes(shown, config, orSame)) this.io.show(config);
  }

  private take(config: T, orSame = false): boolean {
    if (!this.base || !takes(this.base, config, orSame)) return false;
    this.base = config;
    return true;
  }

  private async reread() {
    try {
      this.lastTaken = this.take(await this.io.read(), true);
    } catch {
      // What is shown stands; the next write answers with the right one.
    }
  }

  private async run() {
    this.running = true;
    try {
      for (;;) {
        const write = this.queue.shift();
        if (write) {
          await this.send(write);
          this.io.show(onTop(this.base!, this.queue));
          for (const resolve of this.waiting.get(write) ?? []) resolve();
          this.waiting.delete(write);
        } else if (this.held) {
          const held = this.held;
          this.held = null;
          if (this.lastTaken) this.take(held);
          else await this.reread();
          this.io.show(onTop(this.base!, this.queue));
        } else {
          break;
        }
      }
    } finally {
      this.running = false;
    }
  }

  private async send(write: Write<T>) {
    const base = this.base!;
    try {
      const answer = await write.send(write.change(base), base);
      if (answer) this.lastTaken = this.take(answer);
    } catch (err) {
      this.lastTaken = false;
      if (write.failed?.(err)) await this.reread();
    }
  }
}
