import { useCallback, useLayoutEffect, useRef } from "react";

/**
 * A function that keeps its identity for the component's whole life but always
 * calls the latest `fn` — for handlers handed to memoised children, which
 * would otherwise re-render whenever the parent does just because the closure
 * is new. Only for event handlers: calling it while rendering would run the
 * previous render's `fn`.
 */
export function useStableHandler<A extends unknown[], R>(fn: (...args: A) => R): (...args: A) => R {
  const latest = useRef(fn);
  useLayoutEffect(() => {
    latest.current = fn;
  });
  return useCallback((...args: A) => latest.current(...args), []);
}
