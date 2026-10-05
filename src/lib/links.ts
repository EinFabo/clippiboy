import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { inTauri } from "./ipc";

/**
 * Share links: a clip on clippiboy.com/c/<id> for five days. The core
 * (`links.rs`) uploads and remembers them; this mirrors what it knows for the
 * clip menu and shows the upload as it runs.
 */
export interface Link {
  id: string;
  url: string;
  /** ms since the epoch. */
  expiresAt: number;
}

export interface LinkQuota {
  used: number;
  limit: number;
  resetsAt: number | null;
  full: boolean;
}

interface LinkState {
  signedIn: boolean;
  quota: LinkQuota | null;
  links: Record<string, Link>;
}

export interface LinkProgress {
  clipId: string;
  title: string;
  stage: "shrinking" | "uploading" | "done" | "failed";
  progress: number;
  error: string | null;
}

export const linkApi = {
  state: () => invoke<LinkState>("link_state"),
  /** Uploads, or hands out the live link again; either way it is copied. */
  create: (id: string) => invoke<Link>("link_create", { id }),
  remove: (id: string) => invoke<void>("link_delete", { id }),
};

interface LinksStore extends LinkState {
  /** Uploads by clip id — running ones and finished ones until dismissed. */
  uploads: Record<string, LinkProgress>;
  refresh: () => Promise<void>;
  create: (id: string, title: string) => Promise<void>;
  remove: (id: string) => Promise<void>;
  dismiss: (id: string) => void;
}

export const useLinks = create<LinksStore>((set, get) => ({
  signedIn: false,
  quota: null,
  links: {},
  uploads: {},
  async refresh() {
    if (!inTauri) return;
    try {
      set(await linkApi.state());
    } catch (err) {
      console.error("link state", err);
    }
  },
  async create(id, title) {
    try {
      await linkApi.create(id);
    } catch (err) {
      // Refused before the upload began (signed out, say): no progress came,
      // so the card says it instead.
      if (!get().uploads[id]) {
        set((s) => ({
          uploads: {
            ...s.uploads,
            [id]: { clipId: id, title, stage: "failed", progress: 0, error: String(err) },
          },
        }));
      }
    }
    void get().refresh();
  },
  async remove(id) {
    try {
      await linkApi.remove(id);
    } catch (err) {
      // The core already showed it as a notice.
      console.error("link delete", err);
    } finally {
      void get().refresh();
    }
  },
  dismiss(id) {
    set((s) => {
      const uploads = { ...s.uploads };
      delete uploads[id];
      return { uploads };
    });
  },
}));

/** "5 days left", "1 day left", "5 hours left" — rounded like the server's
    page and toast, so a fresh link says five days here too. */
export function timeLeft(link: Link): string {
  const left = link.expiresAt - Date.now();
  const days = Math.round(left / 86_400_000);
  if (days >= 2) return `${days} days left`;
  if (days === 1) return "1 day left";
  const hours = Math.floor(left / 3_600_000);
  if (hours >= 2) return `${hours} hours left`;
  if (hours === 1) return "1 hour left";
  return "a few minutes left";
}

/** The weekday a used-up week frees its next link, "Fri". */
export function freeOn(quota: LinkQuota): string | null {
  if (quota.resetsAt === null) return null;
  return new Date(quota.resetsAt).toLocaleDateString("en-US", { weekday: "short" });
}

if (inTauri) {
  void listen<LinkProgress>("link-progress", (event) =>
    useLinks.setState((s) => ({ uploads: { ...s.uploads, [event.payload.clipId]: event.payload } })),
  );
  void listen("links-changed", () => void useLinks.getState().refresh());
  // Signing in or out changes what the menu may offer. Only that: the same
  // event carries every friend's presence and comes often.
  void listen<{ signedIn: boolean }>("friends-state", (event) => {
    if (event.payload.signedIn !== useLinks.getState().signedIn) void useLinks.getState().refresh();
  });
  void useLinks.getState().refresh();
  // A link runs out on its own, with no event to say so: dropped here within
  // the minute, so the tile's pill and the menu let go of it. The server is
  // asked again now and then, for a week that has reset meanwhile.
  window.setInterval(() => {
    const { links } = useLinks.getState();
    const now = Date.now();
    const live = Object.fromEntries(Object.entries(links).filter(([, link]) => link.expiresAt > now));
    if (Object.keys(live).length !== Object.keys(links).length) useLinks.setState({ links: live });
  }, 30_000);
  window.setInterval(() => void useLinks.getState().refresh(), 10 * 60_000);
}
