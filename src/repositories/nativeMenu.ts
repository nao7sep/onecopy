// Serialize availability across Main effect replacements. Only the newest
// desired value waits behind an in-flight IPC call; modal nesting cannot grow
// a queue or let an old cleanup disable a newly mounted command owner.
import { invoke } from "@tauri-apps/api/core";
import { reportWindowCall } from "./logging";

let pending: boolean | undefined;
let running = false;

export function setMainMenuAvailable(enabled: boolean): void {
  pending = enabled;
  if (running) return;
  running = true;
  void (async () => {
    while (pending !== undefined) {
      const value = pending;
      pending = undefined;
      try {
        await invoke("set_main_menu_available", { enabled: value });
      } catch (error) {
        reportWindowCall("set native menu availability")(error);
      }
    }
    running = false;
  })();
}
