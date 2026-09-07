import type { WebviewWindow } from "@tauri-apps/api/webviewWindow";

const WINDOW_CREATION_TIMEOUT_MS = 10_000;

/** Waits for one native window-construction outcome and releases both
 * one-shot registrations on every terminal path. */
export function waitForWindowCreated(
  window: WebviewWindow,
  label: string,
): Promise<void> {
  return new Promise((resolve, reject) => {
    let settled = false;
    let stops: Array<() => void> = [];
    const finish = (outcome: { ok: true } | { ok: false; error: unknown }) => {
      if (settled) return;
      settled = true;
      globalThis.clearTimeout(timer);
      for (const stop of stops.splice(0).reverse()) stop();
      if (outcome.ok) resolve();
      else reject(outcome.error);
    };
    const timer = globalThis.setTimeout(
      () =>
        finish({
          ok: false,
          error: new Error(`${label} window creation timed out`),
        }),
      WINDOW_CREATION_TIMEOUT_MS,
    );
    const registrations = [
      window.once("tauri://created", () => finish({ ok: true })),
      window.once("tauri://error", (event) =>
        finish({ ok: false, error: event.payload }),
      ),
    ];
    for (const registration of registrations) {
      void registration.then(
        (stop) => {
          if (settled) stop();
          else stops.push(stop);
        },
        (error) => finish({ ok: false, error }),
      );
    }
  });
}
