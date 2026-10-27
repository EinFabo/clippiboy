import { Component, type ReactNode } from "react";
import { shouldReset } from "@/lib/uiError";

/** What a render error leaves behind instead of a blank window. The core is
 *  told by the root's `onCaughtError`. `compact` is the in-game banner: no
 *  box, nothing to click. `overlay` is for the portalled viewers: the message
 *  sits fixed and centred where the viewer was, not inline below the page. */
export class ErrorBoundary extends Component<
  { onBack?: () => void; compact?: boolean; overlay?: boolean; resetKey?: unknown; children?: ReactNode },
  { failed: boolean }
> {
  state = { failed: false };

  static getDerivedStateFromError() {
    return { failed: true };
  }

  componentDidUpdate(prev: { resetKey?: unknown }) {
    if (shouldReset(this.state.failed, prev.resetKey, this.props.resetKey)) {
      this.setState({ failed: false });
    }
  }

  render() {
    if (!this.state.failed) return this.props.children;
    // Over a game a crash must not put anything on screen.
    if (this.props.compact) return null;
    return (
      <div
        role="alert"
        className={`${
          this.props.overlay
            ? "fixed left-1/2 top-1/2 z-[100] w-80 -translate-x-1/2 -translate-y-1/2 bg-neutral-900"
            : "mx-auto my-16 max-w-sm bg-black/40"
        } flex flex-col items-center gap-3 rounded-lg border border-white/10 p-6 text-center text-sm text-white`}
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
