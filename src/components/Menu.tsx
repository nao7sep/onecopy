// The app's one menu layer (composite-control conventions): hand-rolled on
// the app's own focus helpers — no menu dependency — so every dropdown
// behaves identically. The Menu is NOT a modal: its Escape / outside-close is
// self-contained and never enters the modal stack (a menu closing with Escape
// must not be tangled with dialogs closing with Escape — the exact bug the
// modal stack fixed elsewhere).
//
// Contract: the trigger is the single tab stop; opening moves focus to the
// first eligible item (last for ArrowUp opening); Up/Down/Home/End move
// (wrapping, the menu behavior); type-ahead
// jumps by label; Enter/Space activate; Escape closes and returns focus to
// the trigger; Tab closes (a menu is navigated with arrows, never Tab);
// outside mousedown closes WITHOUT refocusing (a pointer interaction).
// Non-menuitem children (the zoom stepper) are skipped by arrow navigation
// because items are discovered via [role="menuitem"].

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";
import { isComposingEvent } from "../hooks/useComposing";

/** Lets an item close its menu (with focus restored to the trigger) before
 * running its action — provided by Menu, consumed by MenuItem. */
const MenuCloseContext = createContext<() => void>(() => {});

const VIEWPORT_GAP = 8;
const TRIGGER_GAP = 4;
// The menu's narrow cap (composite-control conventions): as wide as its
// longest command up to here. Every command fits under it in the longest
// language — "Volver a ejecutar el asistente de configuración" measured about
// 300px of label in the macOS UI font, and Segoe UI runs a little wider — so
// only a viewport narrower than the cap cuts a label, with an ellipsis.
const MENU_MAX_WIDTH = 384;

interface MenuPosition {
  top: number;
  left: number;
  maxHeight: number;
  maxWidth: number;
}

function samePosition(a: MenuPosition | undefined, b: MenuPosition) {
  return (
    a?.top === b.top &&
    a.left === b.left &&
    a.maxHeight === b.maxHeight &&
    a.maxWidth === b.maxWidth
  );
}

export function Menu({
  trigger,
  align = "start",
  ariaLabel,
  children,
}: {
  /** Renders the trigger; spread the given props onto the app's own button. */
  trigger: (props: {
    ref: React.RefCallback<HTMLButtonElement>;
    "aria-haspopup": "menu";
    "aria-expanded": boolean;
    onClick: (event: React.MouseEvent<HTMLButtonElement>) => void;
    onKeyDown: (event: React.KeyboardEvent<HTMLButtonElement>) => void;
  }) => React.ReactNode;
  /** Which trigger edge the panel's matching edge sticks to. */
  align?: "start" | "end";
  ariaLabel: string;
  children: React.ReactNode;
}) {
  const [open, setOpen] = useState(false);
  // One local presentation flag: hover never changes it or the target.
  const [keyboardNavigation, setKeyboardNavigation] = useState(false);
  /** Viewport-owned coordinates for the portalled panel. */
  const [position, setPosition] = useState<MenuPosition>();
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const panelRef = useRef<HTMLDivElement | null>(null);
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const activeItem = useRef<HTMLElement | null>(null);
  const openAtEnd = useRef(false);

  const items = () =>
    Array.from(
      panelRef.current?.querySelectorAll<HTMLElement>(
        '[role="menuitem"]:not(:disabled):not([aria-disabled="true"])',
      ) ?? [],
    );

  const close = (refocus: boolean) => {
    setOpen(false);
    activeItem.current = null;
    if (refocus) triggerRef.current?.focus();
  };

  const placePanel = useCallback(() => {
    const trigger = triggerRef.current;
    const panel = panelRef.current;
    if (trigger === null || panel === null) return;

    const triggerRect = trigger.getBoundingClientRect();
    const panelRect = panel.getBoundingClientRect();
    const viewportWidth = window.innerWidth;
    const viewportHeight = window.innerHeight;
    const maxWidth = Math.max(0, Math.min(MENU_MAX_WIDTH, viewportWidth - VIEWPORT_GAP * 2));
    const maxFullHeight = Math.max(0, viewportHeight - VIEWPORT_GAP * 2);
    const visibleWidth = Math.min(panelRect.width, maxWidth);
    const intrinsicHeight = panel.scrollHeight;

    const belowTop = triggerRect.bottom + TRIGGER_GAP;
    const belowHeight = Math.max(0, viewportHeight - VIEWPORT_GAP - belowTop);
    const aboveBottom = triggerRect.top - TRIGGER_GAP;
    const aboveHeight = Math.max(0, aboveBottom - VIEWPORT_GAP);

    let top: number;
    let maxHeight: number;
    if (intrinsicHeight <= belowHeight) {
      top = belowTop;
      maxHeight = belowHeight;
    } else if (intrinsicHeight <= aboveHeight) {
      top = aboveBottom - intrinsicHeight;
      maxHeight = aboveHeight;
    } else {
      // Neither side can hold the full menu. Use the whole viewport and let
      // the one menu surface scroll, so every command remains reachable.
      top = VIEWPORT_GAP;
      maxHeight = maxFullHeight;
    }

    const preferredLeft =
      align === "end" ? triggerRect.right - visibleWidth : triggerRect.left;
    const maxLeft = Math.max(VIEWPORT_GAP, viewportWidth - VIEWPORT_GAP - visibleWidth);
    const left = Math.min(Math.max(preferredLeft, VIEWPORT_GAP), maxLeft);
    const next = { top, left, maxHeight, maxWidth };
    setPosition((current) => (samePosition(current, next) ? current : next));
  }, [align]);

  // Measure after the portalled surface is rendered. Running after every
  // render also catches the in-menu zoom stepper, whose webview zoom changes
  // the panel and viewport without closing the menu.
  useLayoutEffect(() => {
    if (open) placePanel();
  });

  // Positioning makes the popup visible before it can receive focus. Both
  // layout passes settle before paint: navigation is ready immediately, not
  // deferred to a frame that the user's first arrow could overtake.
  useLayoutEffect(() => {
    if (!open || position === undefined || activeItem.current !== null) return;
    const list = items();
    const initial = (openAtEnd.current ? list[list.length - 1] : list[0]) ?? null;
    activeItem.current = initial;
    (initial ?? panelRef.current)?.focus();
  }, [open, position]);

  useEffect(() => {
    if (!open) return;
    const viewport = window.visualViewport;
    const observer =
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver(() => placePanel());
    if (panelRef.current !== null) observer?.observe(panelRef.current);
    window.addEventListener("resize", placePanel);
    viewport?.addEventListener("resize", placePanel);
    return () => {
      observer?.disconnect();
      window.removeEventListener("resize", placePanel);
      viewport?.removeEventListener("resize", placePanel);
    };
  }, [open, placePanel]);

  useEffect(() => {
    if (!open) return;
    const onOutside = (event: MouseEvent) => {
      // The panel is portalled, so "outside" means outside BOTH the trigger
      // wrap and the panel — the two no longer share a DOM subtree.
      const target = event.target as Node;
      if (
        wrapRef.current !== null &&
        !wrapRef.current.contains(target) &&
        panelRef.current !== null &&
        !panelRef.current.contains(target)
      ) {
        close(false); // pointer interaction: no focus restore
      }
    };
    document.addEventListener("mousedown", onOutside);
    return () => {
      document.removeEventListener("mousedown", onOutside);
    };
  }, [open]);

  const onPanelKeyDown = (event: React.KeyboardEvent) => {
    if (isComposingEvent(event)) return;
    setKeyboardNavigation(true);
    // An open menu owns the keyboard for the keys it actually claims below. It
    // is a composite, not a modal (its Escape deliberately stays out of the
    // modal stack), so the main window's command layer is still live behind
    // it — a key this handler does NOT claim must genuinely bubble to reach
    // it. "Backspace" is the reachable case: the single-char type-ahead below
    // skips it (it is 9 characters), so an unclaimed Backspace must still
    // reach the global command layer instead of being silently swallowed here
    // (R8-02) — stopPropagation only runs once a branch below claims the key.
    const list = items();
    const current = Math.max(0, list.indexOf(activeItem.current as HTMLElement));
    const focusAt = (index: number) => {
      if (list.length === 0) return;
      const wrapped = (index + list.length) % list.length;
      activeItem.current = list[wrapped] ?? null;
      activeItem.current?.focus();
    };
    if (event.key === "ArrowDown") {
      event.preventDefault();
      event.stopPropagation();
      focusAt(current + 1);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      event.stopPropagation();
      focusAt(current - 1);
    } else if (event.key === "Home") {
      event.preventDefault();
      event.stopPropagation();
      focusAt(0);
    } else if (event.key === "End") {
      event.preventDefault();
      event.stopPropagation();
      focusAt(list.length - 1);
    } else if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      close(true);
    } else if (event.key === "Tab") {
      event.stopPropagation();
      close(false);
    } else if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      event.stopPropagation();
      (document.activeElement as HTMLElement | null)?.click();
    } else if (event.key.length === 1 && !event.metaKey && !event.ctrlKey && !event.altKey) {
      // Single-char type-ahead with wraparound from the current item. Claimed
      // only when it actually moves focus — an unmatched character (no item
      // starts with it) falls through like any other unclaimed key.
      const typed = event.key.toLowerCase();
      for (let step = 1; step <= list.length; step += 1) {
        const candidate = list[(current + step + list.length) % list.length];
        if (candidate?.textContent?.trim().toLowerCase().startsWith(typed)) {
          event.stopPropagation();
          focusAt((current + step) % list.length);
          break;
        }
      }
    }
  };

  const openMenu = (keyboard: boolean, atEnd = false) => {
    activeItem.current = null;
    openAtEnd.current = atEnd;
    setKeyboardNavigation(keyboard);
    setPosition(undefined);
    setOpen(true);
  };

  const openAtTrigger = (event: React.MouseEvent<HTMLButtonElement>) => {
    if (open) close(false);
    else openMenu(event.detail === 0);
  };

  const onTriggerKeyDown = (event: React.KeyboardEvent<HTMLButtonElement>) => {
    if (open || isComposingEvent(event)) return;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      event.stopPropagation();
      openMenu(true, event.key === "ArrowUp");
    }
  };

  return (
    <div ref={wrapRef} className="relative">
      {trigger({
        ref: (el) => {
          triggerRef.current = el;
        },
        "aria-haspopup": "menu",
        "aria-expanded": open,
        onClick: openAtTrigger,
        onKeyDown: onTriggerKeyDown,
      })}
      {/* PORTALLED to the body: the panel used to be absolutely positioned
          inside its column, whose overflow-hidden clipped it — the menu
          opened UNDER the center pane. A portal with fixed positioning is
          the root fix; z-index inside a clipping ancestor cannot be. */}
      {open
        ? createPortal(
            <MenuCloseContext.Provider value={() => close(true)}>
              <div
                ref={panelRef}
                role="menu"
                tabIndex={-1}
                aria-label={ariaLabel}
                data-keyboard-navigation={keyboardNavigation}
                style={{
                  ...position,
                  visibility: position === undefined ? "hidden" : "visible",
                }}
                // The panel's padding insets every item from its edge, so a
                // highlighted item reads as a rounded row inside the popover
                // rather than a bar across it; the items' corners sit inside
                // the panel's larger one. It scrolls only vertically: a label
                // past the cap ends with an ellipsis instead.
                className="fixed z-40 max-w-[min(24rem,calc(100vw-1rem))] min-w-[min(14rem,calc(100vw-1rem))] overflow-y-auto overflow-x-hidden rounded-xl border border-border bg-surface p-1 shadow-xl"
                onFocus={(event) => {
                  if (event.target instanceof HTMLElement && items().includes(event.target)) {
                    activeItem.current = event.target;
                  }
                }}
                onKeyDown={onPanelKeyDown}
              >
                {children}
              </div>
            </MenuCloseContext.Provider>,
            document.body,
          )
        : null}
    </div>
  );
}

export function MenuItem({
  onSelect,
  disabled = false,
  children,
}: {
  onSelect: () => void;
  disabled?: boolean;
  children: React.ReactNode;
}) {
  const closeMenu = useContext(MenuCloseContext);
  return (
    <button
      role="menuitem"
      tabIndex={-1}
      disabled={disabled}
      // App.css owns the independent hover and keyboard treatments. The
      // keyboard mark is inset so the popup's scrolling cannot clip it. The
      // label never wraps; past the menu's cap it ends with an ellipsis.
      className="oc-menu-item flex w-full items-center whitespace-nowrap rounded-md px-2.5 py-1.5 text-left text-sm text-ink outline-none transition-colors motion-reduce:transition-none disabled:opacity-50"
      onClick={() => {
        // Close first (focus back on the trigger), then act — an action that
        // opens a modal then owns focus from a stable base.
        closeMenu();
        onSelect();
      }}
    >
      <span className="min-w-0 truncate">{children}</span>
    </button>
  );
}

export function MenuSeparator() {
  return <div role="separator" className="mx-1.5 my-1 border-t border-border" />;
}
