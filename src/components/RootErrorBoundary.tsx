import { Component, type ErrorInfo, type ReactNode } from "react";
import { documentTranslator } from "../i18n/I18nContext";
import { message } from "../i18n/translate";
import { log, toErrorFields } from "../repositories";
import { recordInterfaceFailure } from "../utils/failureSurface";
import Button from "./ui/Button";

interface Props {
  children: ReactNode;
  onFailure?: () => void;
}

interface State {
  failed: boolean;
}

/** Keeps a renderer failure visible and recoverable in every OneCopy webview.
 * It sits outside the language provider, so it speaks the language the document
 * last declared. */
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
    recordInterfaceFailure(message("crash.drawingUnfinished"), error);
    this.props.onFailure?.();
  }

  render(): ReactNode {
    if (!this.state.failed) return this.props.children;
    const { t } = documentTranslator();
    return (
      <main className="flex h-screen items-center justify-center bg-background p-6 text-ink">
        <section className="w-full max-w-md rounded-2xl border border-border bg-surface p-6 shadow-xl">
          <h1 className="text-lg font-semibold text-ink-strong">
            {t("crash.needsReload")}
          </h1>
          <p className="mt-2 text-sm leading-relaxed text-ink-muted">
            {t("crash.needsReloadBody")}
          </p>
          <Button variant="primary" size="md" className="mt-5" onClick={() => window.location.reload()}>
            {t("crash.reloadWindow")}
          </Button>
        </section>
      </main>
    );
  }
}
