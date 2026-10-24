import { Component, type ReactNode } from "react";

/** What a render error leaves behind instead of a blank window. The core is
 *  told by the root's `onCaughtError`. `compact` is the in-game banner: no
 *  box, nothing to click. */
export class ErrorBoundary extends Component<
  { onBack?: () => void; compact?: boolean; children?: ReactNode },
  { failed: boolean }
> {
  state = { failed: false };

  static getDerivedStateFromError() {
    return { failed: true };
  }

  render() {
    if (!this.state.failed) return this.props.children;
    // Over a game a crash must not put anything on screen.
    if (this.props.compact) return null;
    return (
      <div
        role="alert"
        className="mx-auto my-16 flex max-w-sm flex-col items-center gap-3 rounded-lg border border-white/10 bg-black/40 p-6 text-center text-sm text-white"
      >
        <p>Something broke here.</p>
        <div className="flex gap-2">
          <button
            type="button"
            className="rounded-md bg-white/15 px-3 py-1.5 hover:bg-white/25"
            onClick={() => location.reload()}
          >
            Reload window
          </button>
          {this.props.onBack && (
            <button
              type="button"
              className="rounded-md bg-white/15 px-3 py-1.5 hover:bg-white/25"
              onClick={() => {
                this.setState({ failed: false });
                this.props.onBack?.();
              }}
            >
              Back
            </button>
          )}
        </div>
      </div>
    );
  }
}
