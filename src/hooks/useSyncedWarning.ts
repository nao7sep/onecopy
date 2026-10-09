// Whether a permanent deletion under review reaches a synced folder (iCloud
// Drive, OneDrive, Dropbox and the like), where the sync service also deletes
// the files from the cloud and the user's other devices. The core answers
// from the items' live copies; until it does, and if it cannot, no warning
// shows: the confirmation itself already says the deletion is permanent.

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { identityFromKey } from "../models/items";
import { log, toErrorFields } from "../repositories";

export function useSyncedWarning(keys: readonly string[] | null): boolean {
  const [synced, setSynced] = useState(false);
  const signature = keys === null ? null : keys.join("\n");
  useEffect(() => {
    setSynced(false);
    if (keys === null || keys.length === 0) return;
    let current = true;
    const items = keys.map((key) => {
      const identity = identityFromKey(key);
      return identity.hash !== null ? { hash: identity.hash } : { pathId: identity.pathId };
    });
    invoke<boolean>("items_in_synced_folders", { items })
      .then((answer) => { if (current) setSynced(answer); })
      .catch((error) => log.warn("synced folder check failed", toErrorFields(error)));
    return () => { current = false; };
    // The signature stands for `keys`' content.
  }, [signature]);
  return synced;
}
