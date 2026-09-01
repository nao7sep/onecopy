// The app's one menu layer (composite-control conventions): hand-rolled on
// the app's own focus helpers — no menu dependency — so every dropdown
// behaves identically. The Menu is NOT a modal: its Escape / outside-close is
// self-contained and never enters the modal stack (a menu closing with Escape
// must not be tangled with dialogs closing with Escape — the exact bug the
// modal stack fixed elsewhere).
//
// Contract: the trigger is the single tab stop; opening moves focus to the
// first item; Up/Down/Home/End move (wrapping, the menu behavior); type-ahead
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
    onClick: () => void;
  }) => React.ReactNode;
  /** Which trigger edge the panel's matching edge sticks to. */
  align?: "start" | "end";
  ariaLabel: string;
  children: React.ReactNode;
}) {
  const [open, setOpen] = useState(false);
  /** Viewport-owned coordinates for the portalled panel. */
  const [position, setPosition] = useState<MenuPosition>();
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const panelRef = useRef<HTMLDivElement | null>(null);
  const wrapRef = useRef<HTMLDivElement | null>(null);

  const items = () =>
    Array.from(
      panelRef.current?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [],
    );

  const close = (refocus: boolean) => {
    setOpen(false);
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
    const maxWidth = Math.max(0, viewportWidth - VIEWPORT_GAP * 2);
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
    const raf = requestAnimationFrame(() => items()[0]?.focus());
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
        setOpen(false); // pointer interaction: no focus restore
      }
    };
    document.addEventListener("mousedown", onOutside);
    return () => {
      cancelAnimationFrame(raf);
      document.removeEventListener("mousedown", onOutside);
    };
  }, [open]);

  const onPanelKeyDown = (event: React.KeyboardEvent) => {
    if (isComposingEvent(event)) return;
    // An open menu owns the keyboard. It is a composite, not a modal (its
    // Escape deliberately stays out of the modal stack), so the main window's
    // command layer is still live behind it — and a key this handler does not
    // claim would reach it. "Backspace" is the reachable case: it is 9
    // characters, so the single-char type-ahead below skips it and the global
    // Backspace trashes the selection while the user is only dismissing a menu.
    event.stopPropagation();
    const list = items();
    const current = list.indexOf(document.activeElement as HTMLElement);
    const focusAt = (index: number) => {
      const wrapped = (index + list.length) % list.length;
      list[wrapped]?.focus();
    };
    if (event.key === "ArrowDown") {
      event.preventDefault();
      focusAt(current + 1);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      focusAt(current - 1);
    } else if (event.key === "Home") {
      event.preventDefault();
      focusAt(0);
    } else if (event.key === "End") {
      event.preventDefault();
      focusAt(list.length - 1);
    } else if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      close(true);
    } else if (event.key === "Tab") {
      setOpen(false);
    } else if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      event.stopPropagation();
      (document.activeElement as HTMLElement | null)?.click();
    } else if (event.key.length === 1 && !event.metaKey && !event.ctrlKey && !event.altKey) {
      // Single-char type-ahead with wraparound from the current item.
      const typed = event.key.toLowerCase();
      for (let step = 1; step <= list.length; step += 1) {
        const candidate = list[(current + step + list.length) % list.length];
        if (candidate?.textContent?.trim().toLowerCase().startsWith(typed)) {
          candidate.focus();
          break;
        }
      }
    }
  };

  const openAtTrigger = () => {
    setPosition(undefined);
    setOpen((current) => !current);
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
                aria-label={ariaLabel}
                style={{
                  ...position,
                  visibility: position === undefined ? "hidden" : "visible",
                }}
                className="fixed z-40 max-w-[calc(100vw-1rem)] min-w-[min(14rem,calc(100vw-1rem))] overflow-auto rounded-lg border border-border bg-surface py-1 shadow-xl"
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
      // Keyboard focus shows as a background fill, not a ring — clip-safe
      // inside the rounded panel (composite-control conventions).
      className="flex w-full items-center whitespace-nowrap px-3 py-1.5 text-left text-sm text-ink outline-none transition-colors hover:bg-surface-muted focus:bg-surface-muted disabled:text-ink-muted"
      onClick={() => {
        // Close first (focus back on the trigger), then act — an action that
        // opens a modal then owns focus from a stable base.
        closeMenu();
        onSelect();
      }}
    >
      {children}
    </button>
  );
}

export function MenuSeparator() {
  return <div role="separator" className="my-1 border-t border-border" />;
}
