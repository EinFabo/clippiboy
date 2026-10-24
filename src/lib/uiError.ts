/** Text for the app log from whatever was thrown: the message and stack only,
 *  never anything of the page's content. Cut here as well as in the core. */
export const UI_ERROR_MAX = 4000;

export function describeError(error: unknown, componentStack?: string | null): string {
  let text: string;
  if (error instanceof Error) {
    text = error.stack || `${error.name}: ${error.message}`;
  } else {
    try {
      text = String(error);
    } catch {
      text = "unprintable error";
    }
  }
  if (componentStack) text += `\nin:${componentStack}`;
  return text.length > UI_ERROR_MAX ? text.slice(0, UI_ERROR_MAX) : text;
}

/** A failed boundary recovers when the thing it shows changes. */
export function shouldReset(failed: boolean, prevKey: unknown, nextKey: unknown): boolean {
  return failed && prevKey !== nextKey;
}
