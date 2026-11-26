import type { Link, LinkProgress, LinkQuota } from "./links";

/**
 * Whether a clip can be shared as a link right now, and if not, why — read
 * the same way by the clip's menu and by Send to a friend, where someone
 * offline has a button that shares the clip as a link instead.
 */
export interface LinkReason {
  /** The clip's link while it lasts: a click copies it again, nothing goes up. */
  live: Link | null;
  /** Why a new one can't be had, for the label ("uploading…"); null when it can. */
  reason: string | null;
  /** Whether a click may go ahead. */
  enabled: boolean;
}

/** What the links store knows, as far as one clip's link goes. */
export interface LinkFacts {
  links: Record<string, Link>;
  signedIn: boolean;
  quota: LinkQuota | null;
  uploads: Record<string, LinkProgress>;
}

/**
 * The store is only fetched on changes, and the app runs for days in the
 * tray: a link may have run out and a used-up week reset since. Those count
 * as gone here. `inApp` is false in the browser, where nothing can go up.
 */
export function linkReason(clipId: string, facts: LinkFacts, now: number, inApp: boolean): LinkReason {
  const stored = facts.links[clipId];
  const live = stored && stored.expiresAt > now ? stored : null;
  if (live) return { live, reason: null, enabled: true };
  const quotaStale = facts.quota?.resetsAt != null && facts.quota.resetsAt <= now;
  const quota = quotaStale ? null : facts.quota;
  const running = facts.uploads[clipId];
  const busy = running !== undefined && (running.stage === "shrinking" || running.stage === "uploading");
  const reason = !inApp
    ? null
    : busy
      ? "uploading…"
      : !facts.signedIn
        ? "sign in on Friends first"
        : quota && quota.used >= quota.limit
          ? `none left until ${freeOn(quota) ?? "next week"}`
          : quota?.full
            ? "sharing is full right now"
            : null;
  // A used-up week stays clickable: what the store knows may be old, and the
  // upload asks the server before it shrinks anything. Not while this clip is
  // on its way up — its own pending upload is what used the week up.
  const usedUp = !busy && quota !== null && quota.used >= quota.limit;
  return { live: null, reason, enabled: inApp && (reason === null || usedUp) };
}

/** "Share as link (5 days)", or with why it can't be had: "Share as link · uploading…". */
export function shareAsLinkLabel(reason: string | null): string {
  return reason ? `Share as link · ${reason}` : "Share as link (5 days)";
}

/** The button beside someone offline in Send to a friend: copy the live link, or make one. */
export function linkActionLabel(link: LinkReason): string {
  return link.live ? "Copy share link" : shareAsLinkLabel(link.reason);
}

/** The weekday a used-up week frees its next link, "Fri". */
export function freeOn(quota: LinkQuota): string | null {
  if (quota.resetsAt === null) return null;
  return new Date(quota.resetsAt).toLocaleDateString("en-US", { weekday: "short" });
}
