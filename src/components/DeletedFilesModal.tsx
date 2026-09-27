// Browse: one configured root's deleted files, as a working surface. The list
// is one multi-select listbox (composite-control conventions) driven by the
// active-descendant technique, because it is virtualized: focus stays on the
// list, arrows move the active row, Space selects, Right/Left expand and
// collapse a deleted item. Rows carry no focusable controls of their own;
// Reveal acts on the active row from the footer.

import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ChevronDown, ChevronRight, Square, SquareCheck, SquareMinus } from "lucide-react";
import { useI18n } from "../i18n/I18nContext";
import { message, type Message } from "../i18n/translate";
import type { MessageKey } from "../i18n/catalogues";
import { useDisplayZone } from "../hooks/useDisplayZone";
import { useComposing } from "../hooks/useComposing";
import { formatBytes } from "../models/items";
import {
  browseRows,
  bucketByDay,
  entryFolder,
  entryName,
  filterGroups,
  groupEntries,
  groupSelected,
  isRestorable,
  reconcileSelection,
  toggleEntry,
  toggleGroup,
  type BrowseRow,
  type DeletionGroup,
  type EntryStatus,
  type TrashEntry,
  type TrashListing,
} from "../models/deletedFiles";
import {
  restoreReceipt,
  type RestoreOutcome,
  type RestoreReview,
} from "../models/deletedFiles";
import { mutationProgressLine, mutationResultLine } from "../models/mutation";
import { useMutationStore } from "../state/mutation-store";
import { recordActionFailure } from "../state/notifications-store";
import { log, toErrorFields } from "../repositories";
import { revealInFileManager } from "../workflows/external-open";
import { fileManagerWord } from "../utils/shortcuts";
import { visibleWindow } from "../utils/virtualize";
import ModalShell from "./ModalShell";
import RestoreReviewModal from "./RestoreReviewModal";
import Button from "./ui/Button";
import { TextInput } from "./ui/Field";
import OperationResult from "./ui/OperationResult";

const ROW_HEIGHT = 48;
/** Stands in for the viewport before layout has measured it. */
const FALLBACK_VIEWPORT = ROW_HEIGHT * 14;

export const STATUS_REASONS: Record<Exclude<EntryStatus, "restorable">, MessageKey> = {
  unverified: "deletedFiles.statusUnverified",
  changed: "deletedFiles.statusChanged",
  "outside-root": "deletedFiles.statusOutsideRoot",
  unrepresentable: "deletedFiles.statusUnrepresentable",
  excluded: "deletedFiles.statusExcluded",
};

let nextRequest = 0;

export default function DeletedFilesModal({
  location,
  onClose,
}: {
  /** The deleted-files location, as `trash_overview` names it. */
  location: string;
  onClose: () => void;
}) {
  const { t, text, number, dateTime, calendarDay } = useI18n();
  useDisplayZone();
  const manager = fileManagerWord();
  const [listing, setListing] = useState<TrashListing | null>(null);
  const [loadError, setLoadError] = useState<Message | null>(null);
  const [error, setError] = useState<Message | null>(null);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set());
  const [activeKey, setActiveKey] = useState<string | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  // The list holds focus on its rows' behalf, so the active row, not the
  // list, shows the keyboard cursor, and only while the list has focus.
  const [listFocused, setListFocused] = useState(false);
  const listRef = useRef<HTMLDivElement>(null);
  const request = useRef(0);
  const { handlers: composingHandlers } = useComposing();
  const [restoring, setRestoring] = useState(false);
  const restoringRef = useRef(false);
  const [pendingReview, setPendingReview] = useState<{
    review: RestoreReview;
    token: string | null;
    changed: boolean;
    ids: string[];
  } | null>(null);
  const [outcome, setOutcome] = useState<RestoreOutcome | null>(null);
  const progress = useMutationStore((state) => state.progress);
  const cancelling = useMutationStore((state) => state.cancelling);

  const load = () => {
    // A late answer for a superseded request (or a closed modal) is dropped.
    const id = ++nextRequest;
    request.current = id;
    setLoadError(null);
    void invoke<TrashListing>("trash_entries", { root: location })
      .then((result) => {
        if (request.current !== id) return;
        setListing(result);
        setSelected((current) => reconcileSelection(current, result.entries));
      })
      .catch((failure) => {
        if (request.current !== id) return;
        log.warn("deleted files listing failed", { location, ...toErrorFields(failure) });
        setLoadError(message("deletedFiles.loadFailed"));
      });
  };

  useEffect(() => {
    load();
    return () => {
      request.current = 0;
    };
  }, [location]);

  const groups = useMemo(() => groupEntries(listing?.entries ?? []), [listing]);
  const visible = useMemo(() => filterGroups(groups, query), [groups, query]);
  const rows = useMemo(
    () => browseRows(bucketByDay(visible), expanded),
    [visible, expanded],
  );
  const navigable = useMemo(
    () => rows.map((row, index) => ({ row, index })).filter(({ row }) => row.type !== "day"),
    [rows],
  );
  const activeIndex = rows.findIndex((row) => row.key === activeKey);
  const activeRow = activeIndex < 0 ? null : rows[activeIndex];

  const viewport = listRef.current?.clientHeight || FALLBACK_VIEWPORT;
  const slice = visibleWindow(scrollTop, viewport, ROW_HEIGHT, rows.length, 4);

  const moveTo = (index: number) => {
    const row = rows[index];
    if (row === undefined) return;
    setActiveKey(row.key);
    const list = listRef.current;
    if (list === null) return;
    const top = index * ROW_HEIGHT;
    const height = list.clientHeight || FALLBACK_VIEWPORT;
    if (top < list.scrollTop) list.scrollTop = top;
    else if (top + ROW_HEIGHT > list.scrollTop + height) list.scrollTop = top + ROW_HEIGHT - height;
    setScrollTop(list.scrollTop);
  };

  const step = (delta: number) => {
    if (navigable.length === 0) return;
    const position = navigable.findIndex(({ index }) => index === activeIndex);
    const next =
      position < 0
        ? delta > 0 ? 0 : navigable.length - 1
        : Math.min(navigable.length - 1, Math.max(0, position + delta));
    moveTo(navigable[next].index);
  };

  const toggleRow = (row: BrowseRow) => {
    if (row.type === "group") setSelected((current) => toggleGroup(row.group, current));
    if (row.type === "entry") setSelected((current) => toggleEntry(row.entry, current));
  };

  const setOpen = (group: DeletionGroup, open: boolean) => {
    setExpanded((current) => {
      const next = new Set(current);
      if (open) next.add(group.key);
      else next.delete(group.key);
      return next;
    });
  };

  const onListKey = (event: React.KeyboardEvent<HTMLDivElement>) => {
    const page = Math.max(1, Math.floor(viewport / ROW_HEIGHT) - 1);
    switch (event.key) {
      case "ArrowDown":
        step(1);
        break;
      case "ArrowUp":
        step(-1);
        break;
      case "PageDown":
        step(page);
        break;
      case "PageUp":
        step(-page);
        break;
      case "Home":
        if (navigable.length > 0) moveTo(navigable[0].index);
        break;
      case "End":
        if (navigable.length > 0) moveTo(navigable[navigable.length - 1].index);
        break;
      case " ":
        if (activeRow !== null) toggleRow(activeRow);
        break;
      case "ArrowRight":
      case "Enter":
        if (activeRow?.type === "group") setOpen(activeRow.group, event.key === "Enter" ? !activeRow.expanded : true);
        break;
      case "ArrowLeft":
        if (activeRow?.type === "group") setOpen(activeRow.group, false);
        if (activeRow?.type === "entry") {
          setOpen(activeRow.group, false);
          setActiveKey(`group:${activeRow.group.key}`);
        }
        break;
      default:
        return;
    }
    event.preventDefault();
    event.stopPropagation();
  };

  const revealActive = () => {
    if (activeRow === null || activeRow.type === "day") return;
    const path =
      activeRow.type === "entry"
        ? activeRow.entry.storedPath
        : activeRow.group.representative.storedPath;
    setError(null);
    void revealInFileManager(path).catch((failure) => {
      log.warn("deleted file reveal failed", { path, ...toErrorFields(failure) });
      setError(message("reveal.revealFailed", { manager }));
    });
  };

  const selectedCount = selected.size;

  // Restore: a clean selection runs at once; one that needs a review comes
  // back with it and its token, and confirming sends exactly the reviewed ids
  // and token. A plan that changed since comes back as a new review.
  const restore = async (ids: string[], token: string | null) => {
    // The busy claim is taken before the first await.
    if (restoringRef.current || ids.length === 0) return;
    restoringRef.current = true;
    setRestoring(true);
    setError(null);
    setOutcome(null);
    try {
      const result = await invoke<RestoreOutcome>("trash_restore", {
        root: location,
        entries: ids,
        planToken: token,
      });
      if (result.requiresReview && result.review !== null) {
        setPendingReview({
          review: result.review,
          token: result.planToken,
          changed: result.planChanged,
          ids,
        });
        return;
      }
      setPendingReview(null);
      setOutcome(result);
      setSelected(new Set());
      load();
    } catch (failure) {
      log.error("restore failed", { location, ...toErrorFields(failure) });
      setPendingReview(null);
      setError(message("deletedFiles.restoreFailed"));
      recordActionFailure("restore-failed", message("deletedFiles.restoreFailed"), failure);
    } finally {
      restoringRef.current = false;
      setRestoring(false);
    }
  };

  const revealRestored = () => {
    const path = outcome?.restored[0];
    if (path === undefined) return;
    setError(null);
    void revealInFileManager(path).catch((failure) => {
      log.warn("restored file reveal failed", { path, ...toErrorFields(failure) });
      setError(message("reveal.revealFailed", { manager }));
    });
  };

  const liveProgress = restoring && progress?.kind === "restore" ? progress : null;
  const optionId = (row: BrowseRow) => `deleted-${encodeURIComponent(row.key)}`;

  const reasonOf = (entries: readonly TrashEntry[]): string | null => {
    const blocked = entries.find((entry) => !isRestorable(entry));
    if (blocked !== undefined && entries.every((entry) => !isRestorable(entry))) {
      return t(STATUS_REASONS[blocked.status as Exclude<EntryStatus, "restorable">]);
    }
    return entries.some((entry) => entry.status === "unverified")
      ? t(STATUS_REASONS.unverified)
      : null;
  };

  const kindLabel = (group: DeletionGroup): string | null => {
    if (group.kind === "overwrite-displaced") return t("deletedFiles.kindReplaced");
    if (group.kind === "move-cleanup") {
      return group.movedTo === null
        ? t("deletedFiles.kindMovedSomewhere")
        : t("deletedFiles.kindMoved", { destination: group.movedTo });
    }
    return null;
  };

  const renderRow = (row: BrowseRow, index: number) => {
    const style = { height: ROW_HEIGHT };
    if (row.type === "day") {
      return (
        <div
          key={row.key}
          role="presentation"
          style={style}
          className="flex items-end px-2 pb-1.5 text-xs font-semibold text-ink-muted"
        >
          {calendarDay(row.day)}
        </div>
      );
    }
    const active = index === activeIndex;
    const entries = row.type === "group" ? row.group.entries : [row.entry];
    const restorable = entries.some(isRestorable);
    const checked =
      row.type === "group" ? groupSelected(row.group, selected) : selected.has(row.entry.id);
    const partial =
      row.type === "group" &&
      !checked &&
      row.group.entries.some((entry) => selected.has(entry.id));
    const Check = checked ? SquareCheck : partial ? SquareMinus : Square;
    const reason = reasonOf(entries);
    const base = `flex cursor-default items-center gap-2 rounded-md px-2 ${
      active ? `bg-surface-muted ${listFocused ? "ring-2 ring-inset ring-primary-ring" : ""}` : ""
    } ${restorable ? "" : "opacity-60"}`;
    if (row.type === "group") {
      const { group } = row;
      const Chevron = row.expanded ? ChevronDown : ChevronRight;
      const folder = entryFolder(group.representative);
      const counts = [
        t("deletedFiles.copies", { count: group.mains }),
        group.companions > 0 ? t("deletedFiles.companions", { count: group.companions }) : null,
        kindLabel(group),
      ].filter((part) => part !== null);
      return (
        <div
          key={row.key}
          id={optionId(row)}
          role="option"
          aria-selected={checked}
          aria-disabled={!restorable || undefined}
          aria-expanded={row.expanded}
          style={style}
          className={base}
          onClick={() => {
            setActiveKey(row.key);
            toggleRow(row);
          }}
        >
          <span
            aria-hidden="true"
            className="flex h-6 w-6 shrink-0 items-center justify-center text-ink-muted"
            onClick={(event) => {
              event.stopPropagation();
              setActiveKey(row.key);
              setOpen(group, !row.expanded);
            }}
          >
            <Chevron size={14} />
          </span>
          <Check size={16} aria-hidden="true" className="shrink-0 text-ink-muted" />
          <span className="min-w-0 flex-1">
            <span className="flex items-baseline gap-2">
              <span className="truncate text-sm text-ink-strong">{entryName(group.representative)}</span>
              <span className="truncate text-xs text-ink-muted">
                {folder === "" ? t("deletedFiles.topFolder") : folder}
              </span>
            </span>
            <span className="block truncate text-xs text-ink-muted">
              {reason === null ? counts.join(" · ") : `${counts.join(" · ")} · ${reason}`}
            </span>
          </span>
          <span className="shrink-0 text-right text-xs tabular-nums text-ink-muted">
            <span className="block">{dateTime(new Date(group.deletedAtUtc))}</span>
            <span className="block">{formatBytes(group.size, number)}</span>
          </span>
        </div>
      );
    }
    const { entry } = row;
    const folder = entryFolder(entry);
    return (
      <div
        key={row.key}
        id={optionId(row)}
        role="option"
        aria-selected={checked}
        aria-disabled={!restorable || undefined}
        style={style}
        className={`${base} pl-10`}
        onClick={() => {
          setActiveKey(row.key);
          toggleRow(row);
        }}
      >
        <Check size={16} aria-hidden="true" className="shrink-0 text-ink-muted" />
        <span className="min-w-0 flex-1">
          <span className="block truncate text-sm text-ink">{entryName(entry)}</span>
          <span className="block truncate text-xs text-ink-muted">
            {[
              folder === "" ? t("deletedFiles.topFolder") : folder,
              entry.role === "companion" ? t("deletedFiles.companionRole") : null,
              reason,
            ]
              .filter((part) => part !== null)
              .join(" · ")}
          </span>
        </span>
        <span className="shrink-0 text-xs tabular-nums text-ink-muted">
          {formatBytes(entry.size, number)}
        </span>
      </div>
    );
  };

  const footnotes: string[] = [];
  if (listing !== null && listing.unrecordedFiles > 0) {
    footnotes.push(t("deletedFiles.unrecorded", { count: listing.unrecordedFiles }));
  }
  if (listing !== null && listing.malformedLines > 0) {
    footnotes.push(t("deletedFiles.malformed", { count: listing.malformedLines }));
  }

  return (
    <ModalShell
      title={t("deletedFiles.title")}
      onClose={onClose}
      closeDisabled={restoring}
      widthClass="w-[min(1100px,calc(100vw-3rem))]"
      footerStart={
        <span className="text-xs text-ink-muted">
          {t("deletedFiles.selectedCount", { count: selectedCount })}
        </span>
      }
      footerResult={
        liveProgress !== null ? (
          <span className="flex items-center gap-3 text-xs tabular-nums text-primary">
            <span className="min-w-0 flex-1">
              {mutationProgressLine(liveProgress, cancelling, t, number)}
            </span>
            <Button
              disabled={cancelling}
              onClick={() => void useMutationStore.getState().cancel()}
            >
              {cancelling ? t("common.cancelling") : t("common.cancel")}
            </Button>
          </span>
        ) : error !== null ? (
          <OperationResult level="error">{text(error)}</OperationResult>
        ) : outcome !== null ? (
          <OperationResult
            level={outcome.failed > 0 || outcome.error !== null ? "warning" : "info"}
            actions={
              outcome.restored.length > 0 ? (
                <Button onClick={revealRestored}>
                  {t("deletedFiles.revealRestored", { manager })}
                </Button>
              ) : undefined
            }
          >
            {mutationResultLine(restoreReceipt(outcome), t)}
          </OperationResult>
        ) : undefined
      }
      primaryAction={
        <>
          <Button
            disabled={activeRow === null || activeRow.type === "day"}
            onClick={revealActive}
          >
            {t("reveal.showIn", { manager })}
          </Button>
          <Button
            variant="primary"
            disabled={restoring || selectedCount === 0}
            onClick={() => void restore([...selected], null)}
          >
            {t("deletedFiles.restore")}
          </Button>
        </>
      }
    >
      {pendingReview !== null ? (
        <RestoreReviewModal
          review={pendingReview.review}
          changed={pendingReview.changed}
          onCancel={() => setPendingReview(null)}
          onConfirm={() => {
            const { ids, token } = pendingReview;
            void restore(ids, token);
          }}
        />
      ) : null}
      <p className="mb-2 select-text break-all text-xs text-ink-muted">{location}</p>
      <TextInput
        type="search"
        value={query}
        className="mb-2 w-full"
        placeholder={t("deletedFiles.searchPlaceholder")}
        aria-label={t("deletedFiles.searchLabel")}
        onChange={(event) => setQuery(event.target.value)}
        {...composingHandlers}
      />
      <div
        ref={listRef}
        role="listbox"
        aria-multiselectable="true"
        aria-label={t("trash.title")}
        aria-activedescendant={activeRow === null ? undefined : optionId(activeRow)}
        tabIndex={0}
        className="h-[52vh] overflow-y-auto rounded-lg border border-border p-1 outline-none"
        onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
        onKeyDown={onListKey}
        onFocus={() => {
          setListFocused(true);
          if (activeRow === null && navigable.length > 0) setActiveKey(navigable[0].row.key);
        }}
        onBlur={() => setListFocused(false)}
      >
        {listing === null ? (
          loadError === null ? (
            <p className="py-4 text-center text-sm text-ink-muted">{t("deletedFiles.loading")}</p>
          ) : (
            <OperationResult level="error" className="m-2">
              {text(loadError)}
            </OperationResult>
          )
        ) : rows.length === 0 ? (
          <p className="py-4 text-center text-sm text-ink-muted">
            {groups.length === 0 ? t("deletedFiles.empty") : t("deletedFiles.noMatches")}
          </p>
        ) : (
          <>
            <div style={{ height: slice.topPad }} />
            {rows.slice(slice.startRow, slice.endRow).map((row, offset) =>
              renderRow(row, slice.startRow + offset),
            )}
            <div style={{ height: slice.bottomPad }} />
          </>
        )}
      </div>
      {footnotes.length === 0 ? null : (
        <p className="mt-2 text-xs text-ink-muted">{footnotes.join(" ")}</p>
      )}
    </ModalShell>
  );
}
