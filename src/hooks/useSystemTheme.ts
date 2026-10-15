import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { log, toErrorFields } from "../repositories";

export type ResolvedTheme = "light" | "dark";

const DARK = "(prefers-color-scheme: dark)";

function pageTheme(): ResolvedTheme {
  return window.matchMedia?.(DARK).matches ? "dark" : "light";
}

/** What the System theme resolves to now, for its Settings entry. The page
 * follows the chosen theme, so while Light or Dark is chosen only the core can
 * read the OS appearance. It is read again when the window regains focus or
 * the page's appearance changes, so the entry follows the OS while shown; the
 * page's own appearance stands in until the first answer. */
export function useSystemTheme(): ResolvedTheme {
  const [theme, setResolved] = useState<ResolvedTheme>(pageTheme);
  useEffect(() => {
    let current = true;
    const read = () => {
      void invoke<ResolvedTheme | null>("system_theme")
        .then((resolved) => {
          if (current && resolved !== null) setResolved(resolved);
        })
        .catch((error) => log.warn("system theme read failed", toErrorFields(error)));
    };
    read();
    const media = window.matchMedia?.(DARK);
    window.addEventListener("focus", read);
    media?.addEventListener("change", read);
    return () => {
      current = false;
      window.removeEventListener("focus", read);
      media?.removeEventListener("change", read);
    };
  }, []);
  return theme;
}
