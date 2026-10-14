// Cmd/Ctrl+W closes the window it is pressed in, the way its close button
// would (keyboard-shortcut-conventions, Closing a window): the close request
// reaches the native close routing, so Main runs its quit, Main in Comparison
// and the Comparison spreads end Comparison, and the fullscreen view closes
// back to Main. Bound in the page of every window: on Windows the framework's
// Close Window item binds only Alt+F4, and on macOS the menu's item cannot
// press the close button a borderless window (the fullscreen view, Comparison)
// does not have. Handling it here keeps the key from reaching that item.
import { getCurrentWindow } from "@tauri-apps/api/window";
import { reportWindowCall } from "../repositories";
import { isCloseWindowShortcut, shadowsMacTextEditing } from "../utils/shortcuts";

export function installCloseWindowShortcut(
  close: () => Promise<void> = () => getCurrentWindow().close(),
): () => void {
  const onKeyDown = (event: KeyboardEvent): void => {
    if (event.defaultPrevented || event.isComposing || !isCloseWindowShortcut(event)) return;
    if (shadowsMacTextEditing(event)) return;
    event.preventDefault();
    // A held chord closes one window, not each one that becomes key after it.
    if (event.repeat) return;
    void close().catch(reportWindowCall("close window"));
  };
  window.addEventListener("keydown", onKeyDown, true);
  return () => window.removeEventListener("keydown", onKeyDown, true);
}
