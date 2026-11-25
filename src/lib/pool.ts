/**
 * `Promise.allSettled` over `items`, with at most `limit` of them running at
 * once — a hundred clips deleted together should not be a hundred file moves
 * started in the same instant. The results stand in the order of `items`.
 */
export async function settleInPool<T, R>(
  items: readonly T[],
  limit: number,
  run: (item: T) => Promise<R>,
): Promise<Array<PromiseSettledResult<R>>> {
  const results = new Array<PromiseSettledResult<R>>(items.length);
  let next = 0;
  const worker = async () => {
    while (next < items.length) {
      const at = next++;
      try {
        results[at] = { status: "fulfilled", value: await run(items[at]) };
      } catch (reason) {
        results[at] = { status: "rejected", reason };
      }
    }
  };
  await Promise.all(Array.from({ length: Math.min(Math.max(1, limit), items.length) }, worker));
  return results;
}
