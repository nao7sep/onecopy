import { useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

// The identify flash: one borderless window per monitor showing that screen's
// priority ordinal, exactly as OS display settings do. Self-closing — it is a
// flash, not a surface; nothing else references these windows.

export default function IdentifyWindow({ number }: { number: number }) {
  useEffect(() => {
    const timer = setTimeout(() => {
      void getCurrentWindow().close().catch(() => {});
    }, 2200);
    return () => clearTimeout(timer);
  }, []);

  return (
    <div className="flex h-screen items-center justify-center bg-primary">
      <span className="text-[9rem] font-bold leading-none text-ink-inverted">{number}</span>
    </div>
  );
}
