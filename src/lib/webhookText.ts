/** What a window knows of the connected Discord channel (`webhook.rs`). */
export interface PostTarget {
  /** The server's name. */
  guild: string | null;
  /** Clips waiting to be posted or going up now. */
  posting: string[];
}

/** The clip menu's label: "Post to <server>", or that it is on its way. */
export function postLabel(target: PostTarget, clipId: string): string {
  const name = target.guild ?? "Discord";
  return target.posting.includes(clipId) ? `Posting to ${name}…` : `Post to ${name}`;
}
