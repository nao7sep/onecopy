// Per-webview half of the backend-owned media-use boundary. Components
// register only live audio/video elements; one release event pauses and
// clears them all, acknowledges after the webview has observed the clear,
// and restores surviving elements when the exact backend operation ends.

import { useCallback, useRef, type MutableRefObject } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { log, toErrorFields } from "./repositories";

interface ReleaseMessage {
  token: number;
  keys: string[];
  restorePlayback?: boolean;
}

interface SavedMedia {
  element: HTMLMediaElement;
  src: string | null;
  time: number;
  playing: boolean;
}

const elements = new Set<HTMLMediaElement>();
let activeToken: number | null = null;
let released: SavedMedia[] = [];
let installation: Promise<void> | null = null;
let lifecycle: Promise<void> = Promise.resolve();
const operations = new Map<number, Promise<void>>();
const resumeWaiters = new Map<number, () => void>();

/** Media pause/load events raised while a backend operation owns the readers
 * are release mechanics, not user playback decisions. */
export function mediaUseActive(): boolean {
  return activeToken !== null;
}

function clearElement(element: HTMLMediaElement): SavedMedia {
  const saved: SavedMedia = {
    element,
    src: element.getAttribute("src"),
    time: Number.isFinite(element.currentTime) ? element.currentTime : 0,
    playing: !element.paused,
  };
  element.pause();
  element.removeAttribute("src");
  try {
    element.load();
  } catch {
    // A detached test/webview element may reject load; clearing `src` is the
    // handle-releasing operation and remains authoritative.
  }
  return saved;
}

function register(element: HTMLMediaElement): () => void {
  elements.add(element);
  if (activeToken !== null) released.push(clearElement(element));
  return () => elements.delete(element);
}

function afterPaint(): Promise<void> {
  return new Promise((resolve) => {
    let settled = false;
    const finish = () => {
      if (settled) return;
      settled = true;
      resolve();
    };
    window.requestAnimationFrame(() => window.requestAnimationFrame(finish));
    window.setTimeout(finish, 50);
  });
}

async function release(message: ReleaseMessage): Promise<void> {
  if (activeToken !== message.token) {
    activeToken = message.token;
    released = [...elements].map(clearElement);
  }
  await afterPaint();
  const resumed = new Promise<void>((resolve) => {
    resumeWaiters.set(message.token, resolve);
  });
  const stillActive = await invoke<boolean>("media_use_released", {
    token: message.token,
  });
  if (!stillActive) resume(message.token, message.restorePlayback !== false);
  await resumed;
}

function resume(token: number, restorePlayback = true): void {
  if (activeToken === token) {
    const saved = released;
    released = [];
    activeToken = null;
    for (const { element, src, time, playing } of saved) {
      if (!element.isConnected || src === null) continue;
      element.setAttribute("src", src);
      const restore = () => {
        if (time > 0) {
          try {
            element.currentTime = time;
          } catch {
            // A removed item or unsupported codec has no seekable timeline.
          }
        }
        if (playing && restorePlayback) {
          void element.play().catch(() => undefined);
        }
      };
      element.addEventListener("loadedmetadata", restore, { once: true });
      try {
        element.load();
      } catch {
        // The next React render still owns the element's final presentation.
      }
    }
  }
  resumeWaiters.get(token)?.();
  resumeWaiters.delete(token);
}

function enqueueRelease(message: ReleaseMessage): Promise<void> {
  const existing = operations.get(message.token);
  if (existing !== undefined) return existing;
  const next = lifecycle.then(() => release(message));
  lifecycle = next.catch((error) => {
    log.error("media release failed", toErrorFields(error));
  });
  const tracked = next.finally(() => operations.delete(message.token));
  operations.set(message.token, tracked);
  return tracked;
}

export function installMediaUseBoundary(): Promise<void> {
  if (installation !== null) return installation;
  installation = (async () => {
    await listen<ReleaseMessage>("media-use://release", ({ payload }) => {
      void enqueueRelease(payload);
    });
    await listen<{ token: number; restorePlayback?: boolean }>(
      "media-use://resume",
      ({ payload }) => {
        resume(payload.token, payload.restorePlayback !== false);
      },
    );
    const current = await invoke<ReleaseMessage | null>("media_use_current");
    if (current !== null) await enqueueRelease(current);
  })();
  return installation;
}

export function useOwnedMedia<T extends HTMLMediaElement>(): readonly [
  MutableRefObject<T | null>,
  (element: T | null) => void,
] {
  const ref = useRef<T | null>(null);
  const unregister = useRef<(() => void) | null>(null);
  const setRef = useCallback((element: T | null) => {
    unregister.current?.();
    unregister.current = null;
    ref.current = element;
    if (element !== null) unregister.current = register(element);
  }, []);
  return [ref, setRef] as const;
}
