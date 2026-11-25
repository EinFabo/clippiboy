// The open clip player or picture viewer, as a gate for ways out that do not
// start in it: "Open in the app" from the console, a friend's clip, a link to
// another page. Those used to swap or drop the viewer under unapplied edits or
// a running save.

/** Asked before leaving: calls `go` now, after a question, after a save, or never. */
type Guard = (go: () => void) => void;

let guard: Guard | null = null;

/** Puts the open viewer in front of every outside way out. Returns its release. */
export function guardViewer(next: Guard): () => void {
  guard = next;
  return () => {
    if (guard === next) guard = null;
  };
}

/** Runs `go` once the open viewer lets it — at once when none is open. */
export function whenViewerFree(go: () => void): void {
  if (guard) guard(go);
  else go();
}
