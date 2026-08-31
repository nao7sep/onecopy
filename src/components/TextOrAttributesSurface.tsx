import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ExternalLink, FolderOpen } from "lucide-react";
import type { ItemDetail } from "../models/items";
import { formatBytes } from "../models/items";
import { takenPresentation } from "../models/itemPresentation";
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
  byteSize: number;
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
  const [error, setError] = useState<string | null>(null);
  const [encodings, setEncodings] = useState<string[]>([]);
  const loadedKey = useRef<string | null>(null);

  useEffect(() => {
    void installContentSessionClient().catch(() => undefined);
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
        if (current) setError(String(failure));
      });
    return () => {
      current = false;
    };
  }, [detail, hash, identityKey, pathId, selectedEncoding]);

  const openExternal = () => {
    setError(null);
    void openInDefaultApp(hash, pathId).catch((failure) => {
      log.warn("external open failed", toErrorFields(failure));
      setError("Couldn’t open this file in its default app.");
    });
  };

  if (body === null && error === null) {
    return <p className="text-sm text-ink-muted">Reading preview…</p>;
  }

  if (body?.body === "attributes" || body === null) {
    return (
      <AttributesBodyView
        detail={detail}
        reason={[
          specializedFailure,
          error ?? body?.reason ?? "Text preview is unavailable.",
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
            Encoding
            <select
              className="h-7 rounded border border-border bg-background px-1.5 text-xs text-ink"
              value={selectedEncoding}
              onChange={(event) => setTextEncoding(key, event.target.value)}
            >
              <option value="automatic">
                Automatic{body.body === "text" ? ` (${body.encoding})` : ""}
              </option>
              {encodings.map((encoding) => (
                <option key={encoding} value={encoding}>
                  {encodingLabel(encoding)}
                </option>
              ))}
            </select>
          </label>
          <Button variant="ghost" onClick={() => setTextWrap(!wrap)}>
            Wrap {wrap ? "on" : "off"}
          </Button>
          <Button variant="ghost" onClick={openExternal}>
            <ExternalLink size={13} /> Open in default app
          </Button>
        </span>
      </div>
      {specializedFailure !== null ? (
        <p className="shrink-0 text-xs text-danger">{specializedFailure}</p>
      ) : null}
      {error !== null ? (
        <p className="shrink-0 text-xs text-danger">{error}</p>
      ) : null}
      {body.body === "text" ? (
        <pre
          tabIndex={0}
          className={`min-h-0 flex-1 select-text overflow-auto rounded border border-border bg-background p-3 font-mono text-sm leading-relaxed text-ink ${
            wrap ? "whitespace-pre-wrap break-words" : "whitespace-pre"
          }`}
        >
          {body.text}
        </pre>
      ) : (
        <p className="rounded border border-border bg-background p-3 text-sm text-danger">
          {body.reason}
        </p>
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
  const [revealError, setRevealError] = useState<string | null>(null);
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
            <ExternalLink size={14} /> Open in default app
          </Button>
        </div>
        {revealError !== null ? (
          <p className="mt-2 text-xs text-danger">{revealError}</p>
        ) : null}
        <dl className="mt-5 grid grid-cols-[max-content_minmax(0,1fr)] gap-x-4 gap-y-2 text-sm">
          <dt className="text-ink-muted">Type</dt>
          <dd className="text-ink">{detail.kind}</dd>
          <dt className="text-ink-muted">Size</dt>
          <dd className="text-ink">
            {byteSize === null ? "Unknown" : formatBytes(byteSize)}
          </dd>
          <dt className="text-ink-muted">Date</dt>
          <dd className="text-ink">{takenPresentation(detail)}</dd>
          <dt className="text-ink-muted">Copies</dt>
          <dd>
            <p className="mb-1 text-ink">
              {detail.copyPaths.length.toLocaleString()} exact{" "}
              {detail.copyPaths.length === 1 ? "copy" : "copies"}
            </p>
            <ul className="space-y-1">
              {detail.copyPaths.map((path) => (
                <li key={path} className="flex items-start gap-2">
                  <span className="min-w-0 flex-1 select-text break-all text-ink">
                    {path}
                  </span>
                  <button
                    className="shrink-0 rounded p-1 text-ink-muted hover:bg-surface-muted hover:text-ink"
                    aria-label={`Reveal ${path}`}
                    title="Reveal file"
                    onClick={() => {
                      setRevealError(null);
                      void revealInFileManager(path).catch((failure) => {
                        log.warn("reveal failed", {
                          path,
                          ...toErrorFields(failure),
                        });
                        setRevealError("Couldn’t reveal this copy.");
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
