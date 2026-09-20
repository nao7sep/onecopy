import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { revealInFileManager } from "../workflows/external-open";
import type { ItemDetail } from "../models/items";
import { log, toErrorFields } from "../repositories";
import { fileManagerWord } from "../utils/shortcuts";
import { recordActionFailure } from "../state/notifications-store";
import { useI18n } from "../i18n/I18nContext";
import { message, type Message } from "../i18n/translate";
import ModalShell from "./ModalShell";
import OperationResult from "./ui/OperationResult";

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
  // A descriptor, not a finished sentence: the dialog stays open across a
  // language change.
  const [error, setError] = useState<Message | null>(null);
  const manager = fileManagerWord();
  const { t, text } = useI18n();

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
        recordActionFailure(
          "comparison-copies-load-failed",
          message("reveal.loadCopiesFailedFor", { name: fileName }),
          failure,
        );
        if (current) setError(message("reveal.loadCopiesFailed"));
      });
    return () => {
      current = false;
    };
  }, [hash]);

  return (
    <ModalShell
      title={t("reveal.title", { name: fileName })}
      onClose={onClose}
      footerResult={
        error === null ? undefined : (
          <OperationResult level="error">{text(error)}</OperationResult>
        )
      }
    >
      {paths === null && error === null ? (
        <p className="text-sm text-ink-muted">{t("reveal.loading")}</p>
      ) : paths?.length === 0 ? (
        <p className="text-sm text-ink-muted">{t("reveal.noCopies")}</p>
      ) : (
        <ul className="space-y-2">
          {paths?.map((path) => (
            <li key={path}>
              <button
                className="w-full rounded-lg border border-border px-3 py-2 text-left text-sm text-ink hover:bg-surface-muted"
                title={t("reveal.showIn", { manager })}
                onClick={() => {
                  setError(null);
                  void revealInFileManager(path).catch((failure) => {
                    log.warn("comparison reveal failed", {
                      path,
                      ...toErrorFields(failure),
                    });
                    setError(message("reveal.revealFailed", { manager }));
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
