// The secondary-window handshake.
//
// A window the app opens (preview, comparison) renders from events the main
// window sends, and asks for the current state on load by emitting a "ready"
// event. The main window answers immediately — and that answer is the ONLY one
// it will get, because every later broadcast is triggered by a state CHANGE
// that may never come while the user looks at one photo.
//
// So the announcement must not go out until the listener is genuinely
// registered. `listen` is async: emitting on the next line publishes the
// question before anything can hear the answer, and the window then waits
// forever. Both windows had this, and both showed it as an empty placeholder
// that never cleared. It is one bug, so it gets one implementation.

import { emit, listen } from "@tauri-apps/api/event";
import { log, toErrorFields } from "../repositories";
import { presentEscapedFailure, recordInterfaceFailure } from "./failureSurface";

/** Registers `handler` for `channel`, and only then announces on `ready`.
 *
 * Returns a disposer safe to call at any point, including before the
 * registration has resolved. */
export function listenThenAnnounce<T>(
  channel: string,
  ready: string,
  handler: (payload: T) => void,
): () => void {
  let disposed = false;
  let unlisten: (() => void) | null = null;
  void (async () => {
    try {
      const fn = await listen<T>(channel, (event) => handler(event.payload));
      if (disposed) {
        fn();
        return;
      }
      unlisten = fn;
      await emit(ready, {});
    } catch (error) {
      log.error("secondary-window handshake failed", {
        channel,
        ready,
        ...toErrorFields(error),
      });
      const message = error instanceof Error ? error.message : String(error);
      recordInterfaceFailure(message);
      presentEscapedFailure(`This window could not connect to OneCopy: ${message}`);
    }
  })();
  return () => {
    disposed = true;
    unlisten?.();
  };
}
