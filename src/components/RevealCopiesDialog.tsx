import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { revealInFileManager } from "../workflows/external-open";
import type { ItemDetail } from "../models/items";
import { log, toErrorFields } from "../repositories";
import { fileManagerWord } from "../utils/shortcuts";
import { recordInterfaceFailure } from "../utils/failureSurface";
import ModalShell from "./ModalShell";

export default function RevealCopiesDialog({
  hash,
  fileName,
  onClose,
}: {
  hash: string;
  fileName: string;
  onClose: () => void;
}) {
  const [paths, setPaths] = useState<string[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const manager = fileManagerWord();

  useEffect(() => {
    let current = true;
    void invoke<ItemDetail>("get_item_detail", { hash, pathId: null })
      .then((detail) => {
        if (current) setPaths(detail.copyPaths);
      })
      .catch((failure) => {
        log.warn("comparison copy lookup failed", {
          hash,
          ...toErrorFields(failure),
        });
        recordInterfaceFailure(
          `Couldn’t load the physical copies for ${fileName}.`,
        );
        if (current) setError("Couldn’t load the available copies.");
      });
    return () => {
      current = false;
    };
  }, [hash]);

  return (
    <ModalShell
      title={`Show a copy of ${fileName}`}
      onClose={onClose}
      footerStart={error}
    >
      {paths === null && error === null ? (
        <p className="text-sm text-ink-muted">Loading copies…</p>
      ) : paths?.length === 0 ? (
        <p className="text-sm text-ink-muted">No available copy was found.</p>
      ) : (
        <ul className="space-y-2">
          {paths?.map((path) => (
            <li key={path}>
              <button
                className="w-full rounded-lg border border-border px-3 py-2 text-left text-sm text-ink hover:bg-surface-muted"
                title={`Show in ${manager}`}
                onClick={() => {
                  setError(null);
                  void revealInFileManager(path).catch((failure) => {
                    log.warn("comparison reveal failed", {
                      path,
                      ...toErrorFields(failure),
                    });
                    setError(`Couldn’t show this copy in ${manager}.`);
                  });
                }}
              >
                <span className="break-all">{path}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </ModalShell>
  );
}
