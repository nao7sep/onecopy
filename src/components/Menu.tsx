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

import { createContext, useContext, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { isComposingEvent } from "../hooks/useComposing";

/** Lets an item close its menu (with focus restored to the trigger) before
 * running its action — provided by Menu, consumed by MenuItem. */
const MenuCloseContext = createContext<() => void>(() => {});

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
  /** Fixed-position coordinates for the portalled panel, from the trigger's
   * rect at open time. */
  const [position, setPosition] = useState<{ top: number; left?: number; right?: number }>();
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
    const rect = triggerRef.current?.getBoundingClientRect();
    if (rect) {
      setPosition(
        align === "end"
          ? { top: rect.bottom + 4, right: window.innerWidth - rect.right }
          : { top: rect.bottom + 4, left: rect.left },
      );
    }
    setOpen((o) => !o);
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
                style={position}
                className="fixed z-40 min-w-56 rounded-lg border border-border bg-surface py-1 shadow-xl"
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
