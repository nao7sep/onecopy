import { invoke } from "@tauri-apps/api/core";
import { documentTranslator } from "../i18n/I18nContext";
import { message, type Message } from "../i18n/translate";
import { storedValues } from "../i18n/storedValues";

import { mountModalLayer } from "./modalLayer";
import { isTopmostModal, subscribeModalStack } from "./modalStack";

const SURFACE_ID = "onecopy-escaped-failure";

/** Independent, blocking recovery root for errors outside React's render boundary.
 * Continuing is unavailable; its explicit recovery action reloads the window. */
export function presentEscapedFailure(failure: Message): void {
  presentEscapedDetail(documentTranslator().text(failure));
}

/** The same surface for detail OneCopy cannot restate — a condition the core
 * reports as recorded words (`failure://direct`). It shows as it is, in
 * whatever language it arrived in. */
export function presentEscapedDetail(detail: string): void {
  if (typeof document === "undefined") return;
  let surface = document.getElementById(SURFACE_ID);
  if (surface === null) {
    surface = document.createElement("section");
    surface.id = SURFACE_ID;
    surface.setAttribute("role", "alertdialog");
    surface.setAttribute("aria-modal", "true");
    surface.setAttribute("aria-labelledby", `${SURFACE_ID}-title`);
    surface.setAttribute("aria-describedby", `${SURFACE_ID}-detail`);
    Object.assign(surface.style, {
      position: "fixed",
      inset: "16px",
      zIndex: "2147483647",
      display: "flex",
      flexDirection: "column",
      alignItems: "center",
      justifyContent: "center",
      gap: "12px",
      padding: "24px",
      color: "CanvasText",
      background: "Canvas",
      border: "1px solid GrayText",
      borderRadius: "16px",
      textAlign: "center",
    });
    const heading = document.createElement("strong");
    heading.id = `${SURFACE_ID}-title`;
    heading.textContent = documentTranslator().t("crash.needsReload");
    const paragraph = document.createElement("p");
    paragraph.id = `${SURFACE_ID}-detail`;
    paragraph.dataset.failureDetail = "true";
    const reload = document.createElement("button");
    reload.type = "button";
    reload.setAttribute("data-modal-initial-focus", "");
    reload.textContent = documentTranslator().t("crash.reloadWindow");
    reload.addEventListener("click", () => window.location.reload());
    surface.append(heading, paragraph, reload);
    document.body.append(surface);
    const token = {};
    const recovery = surface;
    const unmount = mountModalLayer(recovery, {
      token, recovery: true, onClose: () => undefined, closeDisabled: () => true,
    });
    const unsubscribe = subscribeModalStack(() => {
      if (isTopmostModal(token) && !recovery.contains(document.activeElement)) reload.focus();
    });
    // Reload normally ends this root's lifetime. If its host is removed first,
    // retire its keyboard listener and scroll lock with the DOM it served.
    const removal = new MutationObserver(() => {
      if (recovery.isConnected) return;
      removal.disconnect();
      unsubscribe();
      unmount();
    });
    removal.observe(document.body, { childList: true });
  }
  const target = surface.querySelector<HTMLElement>("[data-failure-detail='true']");
  if (target) target.textContent = detail;
  surface.querySelector<HTMLElement>("[data-modal-initial-focus]")?.focus();
}

/** Persists one current interface condition per webview when the core remains reachable.
 *
 * The condition crosses IPC as its catalogue key and values, so the notice
 * shows it in whatever language is in effect when it is read, and `error`,
 * when the caller has one, as the recorded detail after that sentence. */
export function recordInterfaceFailure(failure: Message, error?: unknown): void {
  void invoke("record_interface_failure", {
    messageKey: failure.key,
    messageValues: storedValues(failure.values) ?? null,
    detail: error === undefined ? null : errorDetail(error),
  }).catch(() => {
    // A failed IPC call means the promised durable Issue does not exist. The
    // DOM is the final independent channel in this webview; do not recurse.
    presentEscapedFailure(message("crash.notSaved", { failure }));
  });
}

function errorDetail(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  try {
    return JSON.stringify(error) ?? String(error);
  } catch {
    return String(error);
  }
}
