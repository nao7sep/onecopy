import { invoke } from "@tauri-apps/api/core";
import { newActivityOperationId, recordActivity } from "./activity";

interface RequestedPreviewResult {
  canonicalHash: string;
  coalesced: boolean;
}

/** The typed webview edge for the core-owned requested-preview single flight. */
export function ensureRequestedPreview(hash: string): Promise<string> {
  const operationId = newActivityOperationId("preview");
  recordActivity({
    kind: "started",
    owner: "preview",
    operationId,
    current: "running",
    reason: "user",
  });
  return invoke<RequestedPreviewResult>("ensure_preview", { hash })
    .then((result) => {
      recordActivity({
        kind: result.coalesced ? "coalesced" : "completed",
        owner: "preview",
        operationId,
        previous: "running",
        current: result.coalesced ? "coalesced" : "succeeded",
        reason: result.coalesced ? "superseded" : "completion",
      });
      return result.canonicalHash;
    })
    .catch((error: unknown) => {
      recordActivity({
        kind: "failed",
        owner: "preview",
        operationId,
        previous: "running",
        current: "failed",
        reason: "error",
      });
      throw error;
    });
}
