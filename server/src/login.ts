// Sign-in bookkeeping that needs no Worker runtime.

/** How many sign-in starts one browser may have waiting at once. */
export const STATES_KEPT = 3;

/** Compares without leaking where the first difference sits. */
export function sameText(a: string, b: string): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return diff === 0;
}

/** The states the browser's cookie holds, oldest first. */
export function statesIn(cookieHeader: string | null, name: string): string[] {
  for (const part of (cookieHeader ?? "").split(";")) {
    const [key, ...rest] = part.trim().split("=");
    if (key === name) return rest.join("=").split(".").filter(Boolean);
  }
  return [];
}

/** The cookie value after another start: newest last, at most STATES_KEPT (base64url has no `.`). */
export function withState(states: string[], state: string): string {
  return [...states, state].slice(-STATES_KEPT).join(".");
}
