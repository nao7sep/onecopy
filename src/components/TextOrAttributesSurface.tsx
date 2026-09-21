import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ExternalLink, FolderOpen } from "lucide-react";
import type { MessageKey } from "../i18n/catalogues";
import { useI18n } from "../i18n/I18nContext";
import { message, type Message, type Translator } from "../i18n/translate";
import type { ItemDetail } from "../models/items";
import { formatBytes } from "../models/items";
import { takenPresentation } from "../models/itemPresentation";
import { useDisplayZone } from "../hooks/useDisplayZone";
import { textEncodingKey } from "../models/contentSession";
import { log, toErrorFields } from "../repositories";
import {
  installContentSessionClient,
  setTextEncoding,
  setTextWrap,
  useContentSessionStore,
} from "../state/content-session-store";
import Button from "./ui/Button";
import { openInDefaultApp, revealInFileManager } from "../workflows/external-open";
import OperationResult from "./ui/OperationResult";
import { recordActionFailure } from "../state/notifications-store";

interface TextBody {
  body: "text";
  text: string;
  encoding: string;
  contentKey: string;
  encodings: string[];
  byteSize: number;
}

interface AttributesBody {
  body: "attributes";
  reason: string;
  // The condition, when the core authored the reason itself; a decode or I/O
  // diagnostic has none and shows as recorded.
  reasonCode: string | null;
  reasonBytes: number | null;
  byteSize: number;
}

function previewReason(
  body: PreviewBody | null | undefined,
  t: Translator["t"],
): string | null {
  if (body === null || body === undefined) return null;
  if (body.body !== "attributes") return body.body === "decodeError" ? body.reason : null;
  if (body.reasonCode === "preview-too-large") {
    return t("reason.previewTooLarge", { bytes: body.reasonBytes ?? 0 });
  }
  if (body.reasonCode === "preview-binary") return t("reason.previewBinary");
  return body.reason;
}

interface DecodeErrorBody {
  body: "decodeError";
  reason: string;
  contentKey: string;
  encodings: string[];
  byteSize: number;
}

type PreviewBody = TextBody | AttributesBody | DecodeErrorBody;

const ENCODING_ALIASES: Record<string, string> = {
  "utf-8": "UTF8",
  shift_jis: "Shift JIS, SJIS, Windows-31J",
  "euc-jp": "EUC-JP",
  "iso-2022-jp": "JIS",
  gbk: "CP936",
  gb18030: "Chinese national standard",
  big5: "Big-5",
  "windows-1252": "CP1252, Western Latin",
  "windows-1251": "CP1251, Cyrillic",
  "windows-1250": "CP1250, Central European",
  ibm866: "CP866",
};

function encodingLabel(encoding: string): string {
  const aliases = ENCODING_ALIASES[encoding];
  return aliases === undefined ? encoding : `${encoding} — ${aliases}`;
}

type SessionOwner = "installation" | "encoding" | "wrap";

// Which pending change failed; each owner names itself in its dismiss label.
const SESSION_DISMISS_LABEL: Record<SessionOwner, MessageKey> = {
  installation: "common.closeInstallationResult",
  encoding: "textPreview.closeEncodingResult",
  wrap: "textPreview.closeWrapResult",
};

function identityPayload(hash: string | null, pathId: number | null) {
  return { hash, pathId: hash === null ? pathId : null };
}

export default function TextOrAttributesSurface({
  hash,
  pathId,
  detail,
  specializedFailure = null,
}: {
  hash: string | null;
  pathId: number | null;
  detail: ItemDetail;
  specializedFailure?: string | null;
}) {
  const { t, text } = useI18n();
  const identityKey = textEncodingKey(hash, pathId);
  const [body, setBody] = useState<PreviewBody | null>(null);
  const key =
    body?.body === "text" || body?.body === "decodeError"
      ? body.contentKey
      : identityKey;
  const selectedEncoding = useContentSessionStore(
    (state) => state.textEncodings[key] ?? "automatic",
  );
  const wrap = useContentSessionStore((state) => state.textWrap);
  const [error, setError] = useState<Message | null>(null);
  const [sessionErrors, setSessionErrors] = useState<Partial<Record<SessionOwner, Message>>>({});
  const sessionAttempts = useRef({ encoding: 0, wrap: 0 });
  const [encodings, setEncodings] = useState<string[]>([]);
  const loadedKey = useRef<string | null>(null);

  const reportSessionFailure = (
    owner: SessionOwner,
    kind: string,
    reason: Message,
    failure: unknown,
  ) => {
    log.warn("content session change failed", { kind, ...toErrorFields(failure) });
    setSessionErrors((current) => ({ ...current, installation: undefined, [owner]: reason }));
    recordActionFailure(kind, reason, failure);
  };

  useEffect(() => {
    let active = true;
    void installContentSessionClient().catch(() => {
      if (active) {
        setSessionErrors((current) => ({
          ...current,
          installation: message("textPreview.settingsSyncFailed"),
        }));
      }
    });
    return () => { active = false; };
  }, []);

  useEffect(() => {
    let current = true;
    if (loadedKey.current !== identityKey) {
      loadedKey.current = identityKey;
      setBody(null);
      setEncodings([]);
    }
    setError(null);
    void invoke<PreviewBody>("text_preview", {
      ...identityPayload(hash, pathId),
      encoding: selectedEncoding === "automatic" ? null : selectedEncoding,
    })
      .then((result) => {
        if (!current) return;
        setBody(result);
        if (result.body !== "attributes") setEncodings(result.encodings);
      })
      .catch((failure) => {
        log.warn("text preview failed", toErrorFields(failure));
        if (current) {
          setError(message("textPreview.prepareFailed"));
        }
      });
    return () => {
      current = false;
    };
  }, [detail, hash, identityKey, pathId, selectedEncoding]);

  const openExternal = () => {
    setError(null);
    void openInDefaultApp(hash, pathId).catch((failure) => {
      log.warn("external open failed", toErrorFields(failure));
      setError(message("textPreview.openFailed"));
    });
  };

  if (body === null && error === null) {
    return <p className="text-sm text-ink-muted">{t("textPreview.reading")}</p>;
  }

  if (body?.body === "attributes" || body === null) {
    return (
      <AttributesBodyView
        detail={detail}
        // A condition the core named is said here; a diagnostic it recorded,
        // and a specialized surface's failure, show as they came.
        reason={[
          specializedFailure,
          error !== null
            ? text(error)
            : (previewReason(body, t) ?? t("textPreview.unavailable")),
        ]
          .filter((part): part is string => part !== null)
          .join(" ")}
        byteSize={body?.byteSize ?? detail.byteSize}
        onOpen={openExternal}
      />
    );
  }

  return (
    <div className="flex h-full min-h-0 w-full flex-col gap-2 p-3">
      <div className="flex shrink-0 flex-wrap items-center justify-between gap-2">
        <span
          className="min-w-0 truncate text-sm text-ink"
          title={detail.fileName}
        >
          {detail.fileName}
        </span>
        <span className="flex items-center gap-2">
          <label className="flex items-center gap-1 text-xs text-ink-muted">
            {t("textPreview.encoding")}
            <select
              className="h-7 rounded border border-input-border bg-background px-1.5 text-xs text-ink"
              value={selectedEncoding}
              onChange={(event) => {
                const attempt = ++sessionAttempts.current.encoding;
                void setTextEncoding(key, event.target.value)
                  .then(() => {
                    if (sessionAttempts.current.encoding !== attempt) return;
                    setSessionErrors((current) => ({
                      ...current,
                      installation: undefined,
                      encoding: undefined,
                    }));
                  })
                  .catch((failure) => {
                    if (sessionAttempts.current.encoding !== attempt) return;
                    reportSessionFailure(
                      "encoding",
                      "text-encoding-change-failed",
                      message("textPreview.encodingChangeFailed"),
                      failure,
                    );
                  });
              }}
            >
              <option value="automatic">
                {body.body === "text"
                  ? t("textPreview.automaticDetected", {
                      encoding: body.encoding,
                    })
                  : t("textPreview.automatic")}
              </option>
              {/* Encoding names and their alias spellings are identifiers. */}
              {encodings.map((encoding) => (
                <option key={encoding} value={encoding}>
                  {encodingLabel(encoding)}
                </option>
              ))}
            </select>
          </label>
          <Button
            variant="ghost"
            onClick={() => {
              const attempt = ++sessionAttempts.current.wrap;
              void setTextWrap(!wrap)
                .then(() => {
                  if (sessionAttempts.current.wrap !== attempt) return;
                  setSessionErrors((current) => ({
                    ...current,
                    installation: undefined,
                    wrap: undefined,
                  }));
                })
                .catch((failure) => {
                  if (sessionAttempts.current.wrap !== attempt) return;
                  reportSessionFailure(
                    "wrap",
                    "text-wrap-change-failed",
                    message("textPreview.wrapChangeFailed"),
                    failure,
                  );
                });
            }}
          >
            {wrap ? t("textPreview.wrapOn") : t("textPreview.wrapOff")}
          </Button>
          <Button variant="ghost" onClick={openExternal}>
            <ExternalLink size={13} /> {t("preview.openInDefaultApp")}
          </Button>
        </span>
      </div>
      {specializedFailure !== null ? (
        <OperationResult level="error" className="shrink-0">
          {/* Still written by the owning preview surface. */}
          {specializedFailure}
        </OperationResult>
      ) : null}
      {(Object.entries(sessionErrors) as Array<[SessionOwner, Message | undefined]>).map(([owner, reason]) =>
        reason !== undefined ? (
          <OperationResult
            key={owner}
            level="error"
            className="shrink-0"
            onDismiss={() => setSessionErrors((current) => ({ ...current, [owner]: undefined }))}
            dismissLabel={t(SESSION_DISMISS_LABEL[owner])}
          >
            {text(reason)}
          </OperationResult>
        ) : null,
      )}
      {error !== null ? (
        <OperationResult level="error" className="shrink-0">
          {text(error)}
        </OperationResult>
      ) : null}
      {body.body === "text" ? (
        <pre
          tabIndex={0}
          className={`min-h-0 flex-1 select-text overflow-auto rounded border border-border bg-background p-3 font-mono text-sm leading-relaxed text-ink outline-none focus-visible:border-primary-ring ${
            wrap ? "whitespace-pre-wrap break-words" : "whitespace-pre"
          }`}
        >
          {body.text}
        </pre>
      ) : (
        <OperationResult level="error" className="text-sm">
          {/* The decode reason comes from the backend. */}
          {body.reason}
        </OperationResult>
      )}
    </div>
  );
}

function AttributesBodyView({
  detail,
  reason,
  byteSize,
  onOpen,
}: {
  detail: ItemDetail;
  reason: string;
  byteSize: number | null;
  onOpen: () => void;
}) {
  const { t, text, dateTime, number } = useI18n();
  useDisplayZone();
  const [revealError, setRevealError] = useState<Message | null>(null);
  return (
    <div className="h-full w-full overflow-auto p-5">
      <div className="mx-auto max-w-3xl">
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0">
            <h2 className="break-all text-base font-medium text-ink">
              {detail.fileName}
            </h2>
            <p className="mt-1 text-sm text-ink-muted">{reason}</p>
          </div>
          <Button onClick={onOpen}>
            <ExternalLink size={14} /> {t("preview.openInDefaultApp")}
          </Button>
        </div>
        {revealError !== null ? (
          <OperationResult level="error" className="mt-2">
            {text(revealError)}
          </OperationResult>
        ) : null}
        <dl className="mt-5 grid grid-cols-[max-content_minmax(0,1fr)] gap-x-4 gap-y-2 text-sm">
          <dt className="text-ink-muted">{t("common.kind")}</dt>
          {/* The kind word is the core's own, and becomes a code in its pass. */}
          <dd className="text-ink">{detail.kind}</dd>
          <dt className="text-ink-muted">{t("common.size")}</dt>
          <dd className="text-ink">
            {byteSize === null ? t("textPreview.unknown") : formatBytes(byteSize, number)}
          </dd>
          <dt className="text-ink-muted">{t("common.date")}</dt>
          <dd className="text-ink">{takenPresentation(detail, t, dateTime)}</dd>
          <dt className="text-ink-muted">{t("textPreview.copies")}</dt>
          <dd>
            <p className="mb-1 text-ink">
              {t("textPreview.exactCopies", { count: detail.copyPaths.length })}
            </p>
            <ul className="space-y-1">
              {detail.copyPaths.map((path) => (
                <li key={path} className="flex items-start gap-2">
                  <span className="min-w-0 flex-1 select-text break-all text-ink">
                    {path}
                  </span>
                  <button
                    className="shrink-0 rounded p-1 text-ink-muted hover:bg-surface-muted hover:text-ink"
                    aria-label={t("textPreview.revealPath", { path })}
                    title={t("textPreview.revealFile")}
                    onClick={() => {
                      setRevealError(null);
                      void revealInFileManager(path).catch((failure) => {
                        log.warn("reveal failed", {
                          path,
                          ...toErrorFields(failure),
                        });
                        setRevealError(message("metadata.revealFailed"));
                      });
                    }}
                  >
                    <FolderOpen size={14} />
                  </button>
                </li>
              ))}
            </ul>
          </dd>
        </dl>
      </div>
    </div>
  );
}
