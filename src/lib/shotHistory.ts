/**
 * Undo and redo for the screenshot editor's marks.
 *
 * `past` holds the marks as they stood before each change, newest last; `future`
 * what an undo took back, so redo can put it again. A change carrying the same
 * gesture as the one before joins that step: a drag writes on every pointer
 * move, a slider on every notch, and each is still one thing to undo.
 *
 * Any new change throws the redo steps away — they belonged to a version of the
 * picture that no longer leads anywhere.
 */
export class ShotHistory<T> {
  private past: T[] = [];
  private future: T[] = [];
  private gesture: string | null = null;

  /** Keep `before`, the value a change is about to replace. */
  record(before: T, gesture?: string) {
    if (gesture === undefined || gesture !== this.gesture) this.past.push(before);
    this.gesture = gesture ?? null;
    this.future = [];
  }

  /** The value one step back, or `null` at the start; `current` goes to redo. */
  undo(current: T): T | null {
    const previous = this.past.pop();
    if (previous === undefined) return null;
    this.future.push(current);
    this.gesture = null;
    return previous;
  }

  /** The value one step forward again, or `null` if nothing was undone. */
  redo(current: T): T | null {
    const next = this.future.pop();
    if (next === undefined) return null;
    this.past.push(current);
    this.gesture = null;
    return next;
  }

  /** Marks from outside — loaded, saved, thrown away. Nothing to walk there. */
  clear() {
    this.past = [];
    this.future = [];
    this.gesture = null;
  }

  get canUndo() {
    return this.past.length > 0;
  }

  get canRedo() {
    return this.future.length > 0;
  }

  /** As loaded or last saved: nothing is unsaved. */
  get atStart() {
    return this.past.length === 0;
  }
}
