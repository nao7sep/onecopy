import { invoke } from "@tauri-apps/api/core";
import { documentTranslator } from "../i18n/I18nContext";
import { message, type Message } from "../i18n/translate";
import { storedValues } from "../i18n/storedValues";

const SURFACE_ID = "onecopy-escaped-failure";

/** Last-resort UI for errors outside React's render boundary. */
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
    surface.setAttribute("role", "alert");
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
    heading.textContent = documentTranslator().t("crash.needsReload");
    const paragraph = document.createElement("p");
    paragraph.dataset.failureDetail = "true";
    const reload = document.createElement("button");
    reload.type = "button";
    reload.textContent = documentTranslator().t("crash.reloadWindow");
    reload.addEventListener("click", () => window.location.reload());
    surface.append(heading, paragraph, reload);
    document.body.append(surface);
  }
  const target = surface.querySelector<HTMLElement>("[data-failure-detail='true']");
  if (target) target.textContent = detail;
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
