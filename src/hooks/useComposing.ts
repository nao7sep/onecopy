// Composition-aware keyboard event handling for IME support.
//
// When using an IME (e.g. Japanese, Chinese, Korean input), pressing Enter
// confirms character conversion rather than submitting a form. This hook
// tracks composition state so Enter-key handlers can distinguish between
// "user pressed Enter to confirm IME conversion" and "user pressed Enter
// to perform an action."
//
// Three-layer detection strategy (all checked by isComposingKeyboardEvent):
//
//   1. compositionstart/compositionend events set a ref flag. The compositionend
//      handler delays clearing via requestAnimationFrame because Safari/WebKit
//      fires compositionend BEFORE the final keydown event — without the delay,
//      the flag would already be false when the keydown handler runs.
//
//   2. KeyboardEvent.isComposing — broadly supported in modern browsers but
//      unreliable in some WebKit builds (see above), so it serves as fallback.
//
//   3. KeyboardEvent.keyCode === 229 — deprecated but historically the most
//      reliable signal across older Blink and WebKit IME implementations.
//      Browsers set keyCode to 229 for any key press that is part of an
//      active composition.

import { useRef, useCallback } from "react";

interface ComposingHandlers {
  onCompositionStart: () => void;
  onCompositionEnd: () => void;
}

interface UseComposingReturn {
  /** Ref that is true while IME composition is active. */
  composingRef: React.RefObject<boolean>;
  /** Spread these onto the <input> or <textarea>. */
  handlers: ComposingHandlers;
}

export function useComposing(): UseComposingReturn {
  const composingRef = useRef(false);
  const rafIdRef = useRef<number | null>(null);

  const onCompositionStart = useCallback(() => {
    // Cancel any pending clear so a rapid start→end→start doesn't misfire.
    if (rafIdRef.current !== null) {
      cancelAnimationFrame(rafIdRef.current);
      rafIdRef.current = null;
    }
    composingRef.current = true;
  }, []);

  const onCompositionEnd = useCallback(() => {
    // Delay clearing by one animation frame so the subsequent keydown
    // (which Safari delivers after compositionend) still sees the flag.
    rafIdRef.current = requestAnimationFrame(() => {
      composingRef.current = false;
      rafIdRef.current = null;
    });
  }, []);

  return { composingRef, handlers: { onCompositionStart, onCompositionEnd } };
}

/**
 * Event-level composition signals, with no per-input ref. Use this where there is no single
 * composing input to track — notably the app-wide shortcut dispatcher, where the chord's own
 * keydown carries isComposing (verified in WebKit and Blink). Sharing it with the ref-based guard
 * below keeps the global dispatcher and the per-input fields detecting composition identically.
 */
export function isComposingEvent(e: React.KeyboardEvent | KeyboardEvent): boolean {
  // React wraps the native event; normalise access.
  const nativeEvent = "nativeEvent" in e ? e.nativeEvent : e;
  if (nativeEvent.isComposing) return true;

  // Legacy fallback for older IME implementations. Confirmed empirically
  // necessary — without this layer, IME Enter still triggered handlers in
  // some test runs during this hook's implementation.
  //
  // keyCode is deprecated and may eventually be removed from TypeScript's DOM
  // types; the cast below lets the build keep working in that case. At runtime
  // this is also safe: reading a missing property in JavaScript yields
  // undefined rather than throwing, so no try/catch is needed.
  const legacyKeyCode = (nativeEvent as { keyCode?: number }).keyCode;
  return legacyKeyCode === 229;
}

/**
 * Returns true if the keyboard event is part of an active IME composition.
 * Uses the three-layer strategy described at the top of this file.
 */
export function isComposingKeyboardEvent(
  composingRef: React.RefObject<boolean>,
  e: React.KeyboardEvent | KeyboardEvent,
): boolean {
  // The per-input ref covers the WebKit case where compositionend fires before the final keydown
  // (see header); isComposingEvent covers the live event-level signals.
  return composingRef.current || isComposingEvent(e);
}
