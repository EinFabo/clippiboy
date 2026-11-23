/**
 * What a live region should hold to say `text`. A screen reader speaks only a
 * change: the same text again ("Moved to position 2 of 4" after a second move
 * back and forth) gets a no-break space added — or taken off — so it is heard
 * again.
 */
export function announce(current: string, text: string): string {
  return current === text ? `${text} ` : text;
}
