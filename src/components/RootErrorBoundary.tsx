import { Component, type ErrorInfo, type ReactNode } from "react";
import { log, toErrorFields } from "../repositories";
import { recordInterfaceFailure } from "../utils/failureSurface";

interface Props {
  children: ReactNode;
  onFailure?: () => void;
}

interface State {
  failed: boolean;
}

/** Keeps a renderer failure visible and recoverable in every OneCopy webview. */
export default class RootErrorBoundary extends Component<Props, State> {
  state: State = { failed: false };

  static getDerivedStateFromError(): State {
    return { failed: true };
  }

  componentDidCatch(error: unknown, info: ErrorInfo): void {
    log.error("webview render failed", {
      ...toErrorFields(error),
      componentStack: info.componentStack,
    });
    recordInterfaceFailure("This window could not finish drawing. Reload it before continuing.");
    this.props.onFailure?.();
  }

  render(): ReactNode {
    if (!this.state.failed) return this.props.children;
    return (
      <main className="flex h-screen items-center justify-center bg-background p-6 text-ink">
        <section className="w-full max-w-md rounded-2xl border border-border bg-surface p-6 shadow-xl">
          <h1 className="text-lg font-semibold text-ink-strong">
            OneCopy needs to reload
          </h1>
          <p className="mt-2 text-sm leading-relaxed text-ink-muted">
            This window could not finish drawing. Completed file operations
            remain completed.
          </p>
          <button
            className="mt-5 inline-flex h-9 items-center justify-center rounded-lg bg-primary px-4 text-sm font-medium text-ink-inverted outline-none hover:brightness-110 focus-visible:ring-2 focus-visible:ring-primary-ring"
            onClick={() => window.location.reload()}
          >
            Reload window
          </button>
        </section>
      </main>
    );
  }
}
