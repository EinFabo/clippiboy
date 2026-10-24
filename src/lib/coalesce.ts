/**
 * Wraps a read so that at most one is in flight. Calls made meanwhile cause
 * exactly one more run after it, which sees everything they announced. A run
 * that fails does not block the next one.
 */
export function coalesce(run: () => Promise<unknown>): () => void {
  let running = false;
  let again = false;
  const go = () => {
    running = true;
    again = false;
    run()
      .catch(() => {})
      .finally(() => {
        running = false;
        if (again) go();
      });
  };
  return () => {
    if (running) again = true;
    else go();
  };
}
