import { useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";

// Open deleted-file views follow the mutation owner's events, regardless of
// which window admitted the operation. Reads are single-flight; a burst leaves
// at most one trailing read, and closed/superseded views reject late results.
export function useMutationRefresh(
  scope: string | null,
  read: (current: () => boolean) => Promise<void>,
  failed: (error: unknown) => void,
): () => void {
  const callbacks = useRef({ read, failed });
  callbacks.current = { read, failed };
  const refresh = useRef<() => void>(() => {});
  useEffect(() => {
    if (scope === null) return;
    let alive = true;
    let version = 0;
    let running = false;
    let pending = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const stops: Array<() => void> = [];
    const run = async () => {
      if (!alive) return;
      if (running) { pending = true; return; }
      running = true;
      pending = false;
      const own = version;
      try { await callbacks.current.read(() => alive && version === own); }
      finally {
        running = false;
        if (alive && pending) void run();
      }
    };
    const request = (immediate = false) => {
      if (!alive) return;
      version += 1;
      if (running) { pending = true; return; }
      if (immediate) {
        if (timer !== null) clearTimeout(timer);
        timer = null;
        void run();
        return;
      }
      if (timer !== null) return;
      timer = setTimeout(() => { timer = null; void run(); }, 250);
    };
    refresh.current = () => request(true);
    void (async () => {
      try {
        for (const name of ["mutation://progress", "mutation://done", "mutation://error"]) {
          const stop = await listen<{ phase?: string; filesDone?: number; summary?: unknown }>(name, ({ payload }) => {
            if (name === "mutation://progress" && (!payload.filesDone || payload.phase === "planning")) return;
            if (name === "mutation://done" && payload.summary === null) return;
            request();
          });
          if (!alive) { stop(); return; }
          stops.push(stop);
        }
      } catch (error) {
        if (alive) callbacks.current.failed(error);
      }
      if (alive) void run();
    })();
    return () => {
      alive = false;
      version += 1;
      refresh.current = () => {};
      if (timer !== null) clearTimeout(timer);
      stops.forEach((stop) => stop());
    };
  }, [scope]);
  return () => refresh.current();
}
