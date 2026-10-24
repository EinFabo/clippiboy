/**
 * How long a request may take before the plugin gives up on it.
 *
 * A status question is answered from memory. A press is not: saving a clip
 * muxes a file, stopping a recording waits for the encoder to finish, and the
 * app only answers when that is done. Two seconds for those turned a save that
 * worked into a warning on the key.
 */
export function timeoutFor(method: string, path: string): number {
	if (method === "GET") {
		return 2000;
	}
	switch (path) {
		case "/v1/clip":
			return 60_000;
		case "/v1/screenshot":
			return 30_000;
		case "/v1/buffer":
			return 15_000;
		case "/v1/recording":
			return 600_000;
		default:
			return 15_000;
	}
}

/**
 * "slow" when the app took its time but may well be there, "gone" for every
 * other failure — a refused or reset connection, or an abort from elsewhere.
 */
export function failureOf(error: unknown): "slow" | "gone" {
	return error instanceof Error && error.name === "TimeoutError" ? "slow" : "gone";
}
