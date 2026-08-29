import { invoke } from "@tauri-apps/api/core";

const SURFACE_ID = "onecopy-escaped-failure";

/** Last-resort UI for errors outside React's render boundary. */
export function presentEscapedFailure(message: string): void {
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
    heading.textContent = "OneCopy needs to reload";
    const detail = document.createElement("p");
    detail.dataset.failureDetail = "true";
    const reload = document.createElement("button");
    reload.type = "button";
    reload.textContent = "Reload window";
    reload.addEventListener("click", () => window.location.reload());
    surface.append(heading, detail, reload);
    document.body.append(surface);
  }
  const detail = surface.querySelector<HTMLElement>("[data-failure-detail='true']");
  if (detail) detail.textContent = message;
}

/** Persists one current interface condition per webview when the core remains reachable. */
export function recordInterfaceFailure(message: string): void {
  void invoke("record_interface_failure", { message }).catch(() => {
    // The direct surface already carries the failure. A failed IPC call has no
    // deeper reliable channel inside this webview and must not recurse.
  });
}
