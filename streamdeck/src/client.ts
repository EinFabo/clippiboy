import { readFile } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";

import streamDeck from "@elgato/streamdeck";

/** What `GET /v1/status` answers — see `src-tauri/src/control.rs`. */
export type Status = {
	version: string;
	bufferActive: boolean;
	bufferedSeconds: number;
	bufferSeconds: number;
	clipSeconds: number;
	game: string | null;
	encoder: string | null;
	fps: number;
	droppedFrames: number;
	saving: boolean;
};

/** Manually entered in the property inspector, for the rare case the handshake file cannot be found. */
export type Override = {
	port?: number;
	token?: string;
};

type Connection = {
	port: number;
	token: string;
	/** Where port and token came from — the property inspector reports it. */
	source: "handshake" | "manual";
};

/** What the property inspector shows about the connection. */
export type Reachability = {
	connected: boolean;
	port: number | null;
	source: "handshake" | "manual" | null;
	version: string | null;
	handshakePath: string;
};

/** How often the keys ask how things stand. Fast enough that the buffer level counts up visibly. */
const POLL_MS = 500;

/** ClippiBoy answers from memory; anything past this means it is not there. */
const TIMEOUT_MS = 2000;

/**
 * The bridge to ClippiBoy: one poller for all keys, and one place that knows
 * where the app is listening.
 *
 * Deliberately one poller rather than one per key — four ClippiBoy keys on a
 * page must not mean four requests every half second.
 */
class Client {
	#connection: Connection | null = null;
	#watchers = new Set<(status: Status | null) => void>();
	#timer: NodeJS.Timeout | null = null;
	#status: Status | null = null;
	/** Wall clock of the last handshake read — keeps a dead app from re-reading the file twice a second. */
	#lookedUp = 0;

	/**
	 * Report the status to `watcher` until the returned function is called.
	 *
	 * Polling only runs while somebody is watching: with no ClippiBoy key on the
	 * visible page there is nothing to draw, and asking anyway would just keep
	 * a sleeping machine busy.
	 */
	watch(watcher: (status: Status | null) => void): () => void {
		this.#watchers.add(watcher);
		watcher(this.#status);
		if (this.#timer === null) {
			this.#timer = setInterval(() => void this.#poll(), POLL_MS);
			void this.#poll();
		}
		return () => {
			this.#watchers.delete(watcher);
			if (this.#watchers.size === 0 && this.#timer !== null) {
				clearInterval(this.#timer);
				this.#timer = null;
				// Not remembered across a pause: whoever comes back wants to know
				// how things are now, not how they were when they left.
				this.#status = null;
			}
		};
	}

	/** The last status seen, or `null` while ClippiBoy is not reachable. */
	get status(): Status | null {
		return this.#status;
	}

	/** For the property inspector: what the connection currently looks like. */
	async describe(): Promise<Reachability> {
		// Ask once rather than reporting the last poll: the property inspector is
		// opened precisely when something is not working, and by then the poller
		// may not have been running for minutes.
		const answer = await this.#send("GET", "/v1/status");
		const status = answer !== null && answer.ok ? ((await answer.json().catch(() => null)) as Status | null) : null;
		return {
			connected: status !== null,
			port: this.#connection?.port ?? null,
			source: this.#connection?.source ?? null,
			version: status?.version ?? null,
			handshakePath: handshakePath(),
		};
	}

	/**
	 * Forget where ClippiBoy was — after the port or token was entered by hand,
	 * or after a rejected token.
	 */
	forget(): void {
		this.#connection = null;
		this.#lookedUp = 0;
	}

	/** Press one of the buttons. `null` means ClippiBoy did not answer at all. */
	async post(path: string, body?: unknown): Promise<{ ok: boolean; error?: string } | null> {
		const answer = await this.#send("POST", path, body);
		if (answer === null) {
			return null;
		}
		if (answer.status === 403) {
			// Most likely a regenerated token. The next attempt fetches the new one.
			this.forget();
			return { ok: false, error: "ClippiBoy rejected the token" };
		}
		const payload = (await answer.json().catch(() => null)) as { ok?: boolean; error?: string } | null;
		return { ok: answer.ok && payload?.ok !== false, error: payload?.error };
	}

	async #poll(): Promise<void> {
		const answer = await this.#send("GET", "/v1/status");
		let next: Status | null = null;
		if (answer !== null && answer.ok) {
			next = (await answer.json().catch(() => null)) as Status | null;
		} else if (answer !== null && answer.status === 403) {
			this.forget();
		}

		this.#status = next;
		for (const watcher of this.#watchers) {
			try {
				watcher(next);
			} catch (error) {
				streamDeck.logger.error("a key could not be redrawn", error);
			}
		}
	}

	async #send(method: string, path: string, body?: unknown): Promise<Response | null> {
		const connection = await this.#connect();
		if (connection === null) {
			return null;
		}
		try {
			return await fetch(`http://127.0.0.1:${connection.port}${path}`, {
				method,
				headers: {
					"X-ClippiBoy-Token": connection.token,
					...(body === undefined ? {} : { "Content-Type": "application/json" }),
				},
				body: body === undefined ? undefined : JSON.stringify(body),
				signal: AbortSignal.timeout(TIMEOUT_MS),
			});
		} catch {
			// ClippiBoy is not running, or is busy with something long. Either way
			// the keys show a dash — this is not worth a log line twice a second.
			return null;
		}
	}

	/**
	 * Where ClippiBoy is listening.
	 *
	 * First choice is what the app itself wrote next to its config; only if that
	 * is missing does the manual entry from the property inspector apply. Both
	 * programs run on this machine under this user, so in the normal case nobody
	 * has to set anything up at all.
	 */
	async #connect(): Promise<Connection | null> {
		if (this.#connection !== null) {
			return this.#connection;
		}
		// A missing app must not turn into a file read every half second.
		if (Date.now() - this.#lookedUp < POLL_MS * 4) {
			return null;
		}
		this.#lookedUp = Date.now();

		const handshake = await this.#readHandshake();
		if (handshake !== null) {
			this.#connection = handshake;
			return handshake;
		}

		const manual = await streamDeck.settings.getGlobalSettings<Override>();
		if (manual.port && manual.token) {
			this.#connection = { port: manual.port, token: manual.token, source: "manual" };
			return this.#connection;
		}
		return null;
	}

	async #readHandshake(): Promise<Connection | null> {
		try {
			const text = await readFile(handshakePath(), "utf8");
			const parsed = JSON.parse(text) as Partial<Connection>;
			if (typeof parsed.port === "number" && typeof parsed.token === "string" && parsed.token !== "") {
				return { port: parsed.port, token: parsed.token, source: "handshake" };
			}
		} catch {
			// Not there means ClippiBoy is not installed, or the control port is
			// switched off in its settings. Both are the user's business, not an error.
		}
		return null;
	}
}

/** `%APPDATA%\ClippiBoy\streamdeck.json` — written by `control::write_handoff`. */
export function handshakePath(): string {
	const appData = process.env.APPDATA ?? join(homedir(), "AppData", "Roaming");
	return join(appData, "ClippiBoy", "streamdeck.json");
}

export const clippiboy = new Client();
