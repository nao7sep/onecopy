import { useEffect, useState } from "react";
import { installBinariesEventWiring, useBinariesStore } from "../state/binaries-store";
import { installTranscriptEventWiring } from "../state/transcript-store";

/** Each auxiliary webview has its own stores. Admit live events before its
 * transcript's initial read, including when the fullscreen window is reused. */
export function useSecondaryPreviewFeatures(): boolean {
  const [ready, setReady] = useState(false);
  useEffect(() => {
    let active = true;
    void Promise.all([
      installBinariesEventWiring(),
      installTranscriptEventWiring(),
    ]).then(async () => {
      await useBinariesStore.getState().load();
      if (active) setReady(true);
    });
    return () => { active = false; };
  }, []);
  return ready;
}
