/** What stands for a friend's name while streamer mode is on. */
export const HIDDEN_NAME = "A friend";

/** A friend's name, or {@link HIDDEN_NAME} in streamer mode. */
export function shownName(name: string, streamer: boolean): string {
  return streamer ? HIDDEN_NAME : name;
}

/** One's friend code as the screen shows it: `XXXX-XXXX`, or nothing of it in
 *  streamer mode — copying still hands out the real one. */
export function shownCode(formatted: string, streamer: boolean): string {
  return streamer ? "Code hidden" : formatted;
}
