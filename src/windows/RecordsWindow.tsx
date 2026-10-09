// The Records window: every record in records.sqlite3, newest first, with
// the selected record's full content beside the list. The list is one
// single-select listbox (composite-control conventions) driven by the
// active-descendant technique: focus stays on the list, the arrows move the
// selection, and the selection follows. More records load by themselves as
// the end of the list comes near, and new ones arrive while the window is open.

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
} from "react";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { useI18n } from "../i18n/I18nContext";
import type { Message } from "../i18n/translate";
import { log, reportWindowCall, toErrorFields } from "../repositories";
import {
  onRecordsChanged,
  readRecordDetail,
  readRecordSources,
  readRecordsListWidth,
  readRecordsPage,
  saveRecordsListWidth,
} from "../repositories/records";
import {
  KIND_LABELS,
  LEVEL_FILTER_LABELS,
  LEVEL_LABELS,
  LEVEL_TONES,
  RECORD_KINDS,
  RECORD_LEVEL_FILTERS,
  cursorAfter,
  fieldPresentation,
  mergeNewestPage,
  prettyJson,
  recordKey,
  recordSentence,
  type RecordDetail,
  type RecordKind,
  type RecordLevel,
  type RecordLevelFilter,
  type RecordSources,
  type RecordsQuery,
  type RecordSummary,
} from "../models/records";
import { isComposingEvent } from "../hooks/useComposing";
import { useDisplayZone } from "../hooks/useDisplayZone";
import OperationResult from "../components/ui/OperationResult";
import { Select, TextInput } from "../components/ui/Field";
import {
  RECORDS_LIST_WIDTH,
  SPLITTER_WIDTH,
  clampRecordsListWidth,
  computeRecordsMinHeight,
  computeRecordsMinWidth,
} from "../utils/windowSizing";

/** How far one arrow press moves the Records list's splitter. */
const KEY_RESIZE_STEP = 16;

// The list pane opens at its saved width, so its first frame already has it.
export default function RecordsWindowRoute() {
  const [listWidth, setListWidth] = useState<number | null>(null);

  useEffect(() => {
    void getCurrentWindow()
      .setMinSize(new LogicalSize(computeRecordsMinWidth(), computeRecordsMinHeight()))
      .catch(reportWindowCall("setMinSize"));
  }, []);

  useEffect(() => {
    let cancelled = false;
    void readRecordsListWidth().then(
      (width) => {
        if (cancelled) return;
        setListWidth(
          typeof width === "number" && Number.isFinite(width)
            ? Math.max(RECORDS_LIST_WIDTH.min, Math.min(RECORDS_LIST_WIDTH.max, width))
            : RECORDS_LIST_WIDTH.default,
        );
      },
      (error: unknown) => {
        log.warn("records list width read failed", toErrorFields(error));
        if (!cancelled) setListWidth(RECORDS_LIST_WIDTH.default);
      },
    );
    return () => {
      cancelled = true;
    };
  }, []);

  if (listWidth === null) return <div className="h-screen bg-background" aria-busy="true" />;
  return <RecordsWindow initialListWidth={listWidth} />;
}

type Filters = Omit<RecordsQuery, "after">;

const NO_FILTERS: Filters = { session: null, kind: null, level: null, search: "" };
const SEARCH_DELAY_MS = 300;
// New records are read at most this often while they keep arriving.
const LIVE_INTERVAL_MS = 1000;

type ListState =
  | { status: "loading" }
  | { status: "failed" }
  | { status: "ready"; records: RecordSummary[]; more: boolean; loadingMore: boolean; moreFailed: boolean };

type DetailState =
  | { status: "none" }
  | { status: "loading" }
  | { status: "failed" }
  | { status: "ready"; record: RecordDetail };

type Selection = { kind: RecordKind; id: number };

// Within about one screen of the end of what is loaded.
function nearEnd(scroll: HTMLElement): boolean {
  return scroll.scrollHeight - scroll.scrollTop - scroll.clientHeight <= scroll.clientHeight;
}

function atTop(scroll: HTMLElement): boolean {
  return scroll.scrollTop < 1;
}

function optionId(key: string): string {
  return `record-${key.replace(":", "-")}`;
}

function formatTime(format: Intl.DateTimeFormat, value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : format.format(date);
}

const NAVIGATION_KEYS = new Set(["ArrowDown", "ArrowUp", "PageDown", "PageUp", "Home", "End"]);

export function RecordsWindow({ initialListWidth }: { initialListWidth: number }) {
  const { t, text, locale } = useI18n();
  const [sources, setSources] = useState<RecordSources | null>(null);
  const [filters, setFilters] = useState<Filters>(NO_FILTERS);
  const [searchText, setSearchText] = useState("");
  const [list, setList] = useState<ListState>({ status: "loading" });
  const [selected, setSelected] = useState<Selection | null>(null);
  const [detail, setDetail] = useState<DetailState>({ status: "none" });
  const [listWidth, setListWidth] = useState(initialListWidth);
  const [dragWidth, setDragWidth] = useState<number | null>(null);
  const [shellWidth, setShellWidth] = useState<number | null>(null);
  const listGeneration = useRef(0);
  // The busy claim for the next page (PLAYBOOK, Own the work in flight).
  const fetchingMore = useRef(false);
  // The filters the current list was read for, for the live reads below.
  const filtersRef = useRef(filters);
  // New records arrived while the list was scrolled away from the top.
  const newestPending = useRef(false);
  // A failed read is itself logged as a record, whose signal would start the
  // next read; live reads stop after a failure and resume after a read succeeds.
  const liveSuspended = useRef(false);
  // The selection as last set, for a focus event that arrives before React
  // has rendered a click's selection.
  const selectedRef = useRef<Selection | null>(null);
  const shellRef = useRef<HTMLDivElement | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const dragCleanup = useRef<(() => void) | null>(null);

  // Times follow the computer's zone, and a change of it.
  const zone = useDisplayZone();
  const rowTime = useMemo(
    () => new Intl.DateTimeFormat(locale, { dateStyle: "short", timeStyle: "medium" }),
    // The zone is read by the formatter when it is made.
    [locale, zone],
  );

  // Pane sizing: window-conventions.
  useEffect(() => {
    const shell = shellRef.current;
    if (shell === null) return;
    const measure = () => setShellWidth(shell.clientWidth);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(shell);
    return () => observer.disconnect();
  }, []);
  const shownListWidth =
    shellWidth === null
      ? Math.max(RECORDS_LIST_WIDTH.min, Math.min(RECORDS_LIST_WIDTH.max, dragWidth ?? listWidth))
      : clampRecordsListWidth(dragWidth ?? listWidth, shellWidth);

  useEffect(() => () => dragCleanup.current?.(), []);

  useEffect(() => {
    const timer = setTimeout(() => {
      setFilters((current) => (current.search === searchText ? current : { ...current, search: searchText }));
    }, SEARCH_DELAY_MS);
    return () => clearTimeout(timer);
  }, [searchText]);

  // The launches the filter offers do not change while the window is open:
  // this launch already has records when it opens.
  useEffect(() => {
    let cancelled = false;
    void readRecordSources().then(
      (next) => {
        if (!cancelled) setSources(next);
      },
      (error: unknown) => log.warn("record sources read failed", toErrorFields(error)),
    );
    return () => {
      cancelled = true;
    };
  }, []);

  // A page applies only while the filters it was read for are still the
  // newest ones asked for.
  useEffect(() => {
    filtersRef.current = filters;
    const generation = ++listGeneration.current;
    fetchingMore.current = false;
    newestPending.current = false;
    setList({ status: "loading" });
    void readRecordsPage({ ...filters, after: null }).then(
      (page) => {
        if (generation !== listGeneration.current) return;
        liveSuspended.current = false;
        setList({ status: "ready", records: page.records, more: page.more, loadingMore: false, moreFailed: false });
      },
      (error: unknown) => {
        if (generation !== listGeneration.current) return;
        liveSuspended.current = true;
        log.warn("records read failed", toErrorFields(error));
        setList({ status: "failed" });
      },
    );
  }, [filters]);

  // The newest page read again for new records. It joins the rows already
  // shown rather than replacing them, so the list never falls back to the
  // loading note and the pages already read stay. It reads only refs, so one
  // copy serves the live subscription below.
  const readNewest = useCallback((): void => {
    const generation = listGeneration.current;
    void readRecordsPage({ ...filtersRef.current, after: null }).then(
      (page) => {
        if (generation !== listGeneration.current) return;
        liveSuspended.current = false;
        setList((current) =>
          current.status === "ready"
            ? { ...current, ...mergeNewestPage(current.records, current.more, page) }
            : { status: "ready", records: page.records, more: page.more, loadingMore: false, moreFailed: false },
        );
      },
      (error: unknown) => {
        if (generation !== listGeneration.current) return;
        liveSuspended.current = true;
        log.warn("records read failed", toErrorFields(error));
      },
    );
  }, []);

  // A stored record reaches the list at once while it is scrolled to the top;
  // otherwise it waits until the list is back there, so the list never moves
  // under the reader.
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | null = null;
    let disposed = false;
    let unsubscribe: (() => void) | null = null;
    void onRecordsChanged(() => {
      if (timer !== null || liveSuspended.current) return;
      timer = setTimeout(() => {
        timer = null;
        const scroll = scrollRef.current;
        if (scroll === null || atTop(scroll)) readNewest();
        else newestPending.current = true;
      }, LIVE_INTERVAL_MS);
    }).then(
      (stop) => {
        if (disposed) stop();
        else unsubscribe = stop;
      },
      (error: unknown) => log.warn("records live updates could not start", toErrorFields(error)),
    );
    return () => {
      disposed = true;
      unsubscribe?.();
      if (timer !== null) clearTimeout(timer);
    };
  }, [readNewest]);

  const selectedKey = selected === null ? null : recordKey(selected);

  useEffect(() => {
    if (selected === null) {
      setDetail({ status: "none" });
      return;
    }
    let cancelled = false;
    setDetail({ status: "loading" });
    void readRecordDetail(selected.kind, selected.id).then(
      (record) => {
        if (!cancelled) setDetail(record === null ? { status: "failed" } : { status: "ready", record });
      },
      (error: unknown) => {
        if (cancelled) return;
        log.warn("record read failed", toErrorFields(error));
        setDetail({ status: "failed" });
      },
    );
    return () => {
      cancelled = true;
    };
    // The selection is compared by its key, not by the object holding it.
  }, [selectedKey]);

  // Loading more: composite-control-conventions, Integration Points. A failed
  // page is read again when the end is reached again.
  const loadMore = (): void => {
    if (list.status !== "ready" || !list.more || fetchingMore.current) return;
    fetchingMore.current = true;
    const generation = listGeneration.current;
    setList((current) => (current.status === "ready" ? { ...current, loadingMore: true, moreFailed: false } : current));
    void readRecordsPage({ ...filters, after: cursorAfter(list.records) }).then(
      (page) => {
        if (generation !== listGeneration.current) return;
        fetchingMore.current = false;
        liveSuspended.current = false;
        setList((current) =>
          current.status === "ready"
            ? { ...current, records: [...current.records, ...page.records], more: page.more, loadingMore: false }
            : current,
        );
      },
      (error: unknown) => {
        if (generation !== listGeneration.current) return;
        fetchingMore.current = false;
        liveSuspended.current = true;
        log.warn("records read failed", toErrorFields(error));
        setList((current) => (current.status === "ready" ? { ...current, loadingMore: false, moreFailed: true } : current));
      },
    );
  };

  // A page that leaves the list short of the end reads the next one; a failed
  // page waits for the reader instead.
  useEffect(() => {
    const scroll = scrollRef.current;
    if (list.status !== "ready" || list.loadingMore || list.moreFailed || scroll === null) return;
    if (nearEnd(scroll)) loadMore();
    // Only a new list state can change what is loaded.
  }, [list]);

  const onListScroll = (): void => {
    const scroll = scrollRef.current;
    if (scroll === null) return;
    if (newestPending.current && atTop(scroll)) {
      newestPending.current = false;
      readNewest();
    }
    if (nearEnd(scroll)) loadMore();
  };

  const records = list.status === "ready" ? list.records : [];
  const keys = records.map(recordKey);
  const selectedIndex = selectedKey === null ? -1 : keys.indexOf(selectedKey);

  const select = (record: RecordSummary): void => {
    const next = { kind: record.kind, id: record.id };
    selectedRef.current = next;
    if (recordKey(record) !== selectedKey) setSelected(next);
  };

  // Keeping the active item in view: the minimum scroll, no animation.
  const reveal = (key: string): void => {
    listRef.current?.querySelector<HTMLElement>(`#${CSS.escape(optionId(key))}`)?.scrollIntoView?.({ block: "nearest" });
  };

  // The list is one listbox (composite-control-conventions, Listbox); the
  // selection follows the arrows, and the end of the list loads the next page.
  const onListKeyDown = (event: KeyboardEvent<HTMLDivElement>): void => {
    if (event.metaKey || event.ctrlKey || event.altKey || event.shiftKey || isComposingEvent(event)) return;
    if (!NAVIGATION_KEYS.has(event.key) || records.length === 0) return;
    event.preventDefault();
    const container = listRef.current;
    const firstOption = container?.querySelector<HTMLElement>('[role="option"]');
    const scroll = scrollRef.current;
    const pageStep =
      scroll !== null && firstOption?.offsetHeight ? Math.max(1, Math.floor(scroll.clientHeight / firstOption.offsetHeight)) : 8;
    const last = records.length - 1;
    const current = selectedIndex;
    const target =
      event.key === "Home"
        ? 0
        : event.key === "End"
          ? last
          : current < 0
            ? 0
            : event.key === "ArrowDown"
              ? Math.min(last, current + 1)
              : event.key === "ArrowUp"
                ? Math.max(0, current - 1)
                : event.key === "PageDown"
                  ? Math.min(last, current + pageStep)
                  : Math.max(0, current - pageStep);
    const record = records[target]!;
    select(record);
    reveal(recordKey(record));
    if (target === last && (event.key === "ArrowDown" || event.key === "PageDown" || event.key === "End")) {
      loadMore();
    }
  };

  // Tab into the list with nothing selected selects the first record.
  const onListFocus = (): void => {
    if (selectedRef.current === null && records.length > 0) select(records[0]!);
  };

  // Drag intent: window-conventions, Content-based minimum size. Only a
  // drag's end saves.
  const beginListDrag = (event: ReactMouseEvent): void => {
    event.preventDefault();
    dragCleanup.current?.();
    const startX = event.clientX;
    const startWidth = shownListWidth;
    const widthAt = (clientX: number) => {
      const desired = startWidth + clientX - startX;
      return shellWidth === null
        ? Math.max(RECORDS_LIST_WIDTH.min, Math.min(RECORDS_LIST_WIDTH.max, Math.round(desired)))
        : clampRecordsListWidth(desired, shellWidth);
    };
    document.body.classList.add("col-resizing");
    const cleanup = () => {
      document.body.classList.remove("col-resizing");
      document.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseup", onUp);
      dragCleanup.current = null;
    };
    const onMove = (moveEvent: MouseEvent) => setDragWidth(widthAt(moveEvent.clientX));
    const onUp = (upEvent: MouseEvent) => {
      const width = widthAt(upEvent.clientX);
      cleanup();
      setListWidth(width);
      setDragWidth(null);
      void saveRecordsListWidth(width).catch((error: unknown) =>
        log.warn("records list width save failed", toErrorFields(error)),
      );
    };
    dragCleanup.current = cleanup;
    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
  };

  // Keyboard resizing (BigMouth's splitter): arrows move the width by
  // KEY_RESIZE_STEP, Home and End go to the bounds, and the width is saved
  // once, when the key is released or the splitter loses focus.
  const keyedWidth = useRef<number | null>(null);
  const resizeByKey = (event: KeyboardEvent<HTMLDivElement>): void => {
    // A held key moves on from where the last press left it, rendered or not.
    const from = keyedWidth.current ?? shownListWidth;
    const target =
      event.key === "ArrowLeft" ? from - KEY_RESIZE_STEP
      : event.key === "ArrowRight" ? from + KEY_RESIZE_STEP
      : event.key === "Home" ? RECORDS_LIST_WIDTH.min
      : event.key === "End" ? RECORDS_LIST_WIDTH.max
      : null;
    if (target === null) return;
    event.preventDefault();
    keyedWidth.current = shellWidth === null
      ? Math.max(RECORDS_LIST_WIDTH.min, Math.min(RECORDS_LIST_WIDTH.max, target))
      : clampRecordsListWidth(target, shellWidth);
    setListWidth(keyedWidth.current);
  };
  const commitKeyedWidth = (): void => {
    if (keyedWidth.current === null) return;
    const width = keyedWidth.current;
    keyedWidth.current = null;
    void saveRecordsListWidth(width).catch((error: unknown) =>
      log.warn("records list width save failed", toErrorFields(error)),
    );
  };

  const launchLabel = (session: string): string => {
    const time = formatTime(rowTime, session);
    return session === sources?.currentSession ? t("records.thisLaunch", { time }) : time;
  };

  return (
    <div ref={shellRef} className="flex h-screen overflow-hidden bg-background text-ink">
      <section
        id="records-list-pane"
        data-records-list-pane
        style={{ width: shownListWidth }}
        className="flex shrink-0 flex-col overflow-hidden bg-surface"
        aria-label={t("records.title")}
      >
        <div className="flex flex-col gap-2 border-b border-border p-3">
          <TextInput
            type="search"
            value={searchText}
            className="w-full"
            placeholder={t("records.search")}
            aria-label={t("records.search")}
            onChange={(event) => setSearchText(event.target.value)}
          />
          <FilterSelect
            label={t("records.launch")}
            value={filters.session}
            allLabel={t("records.allLaunches")}
            options={(sources?.sessions ?? []).map((session) => ({ value: session, label: launchLabel(session) }))}
            onChange={(session) => setFilters({ ...filters, session })}
          />
          <div className="flex gap-2">
            <FilterSelect
              label={t("records.kind")}
              value={filters.kind}
              allLabel={t("records.allKinds")}
              options={RECORD_KINDS.map((kind) => ({ value: kind, label: t(KIND_LABELS[kind]) }))}
              onChange={(kind) => setFilters({ ...filters, kind: kind as RecordKind | null })}
            />
            <FilterSelect
              label={t("records.level")}
              value={filters.level}
              allLabel={t("records.allLevels")}
              options={RECORD_LEVEL_FILTERS.map((level) => ({ value: level, label: t(LEVEL_FILTER_LABELS[level]) }))}
              onChange={(level) => setFilters({ ...filters, level: level as RecordLevelFilter | null })}
            />
          </div>
        </div>
        <div
          ref={scrollRef}
          data-records-scroll
          className="relative min-h-0 flex-1 overflow-y-auto p-1"
          aria-busy={list.status === "loading"}
          onScroll={onListScroll}
        >
          {list.status === "failed" ? (
            <OperationResult level="error" className="m-2">{t("records.loadFailed")}</OperationResult>
          ) : list.status === "loading" ? (
            <p className="px-2 py-3 text-sm text-ink-muted">{t("records.loading")}</p>
          ) : records.length === 0 ? (
            <p className="px-2 py-3 text-sm text-ink-muted">{t("records.empty")}</p>
          ) : (
            <div
              ref={listRef}
              role="listbox"
              tabIndex={0}
              aria-label={t("records.title")}
              aria-activedescendant={selectedKey !== null && selectedIndex >= 0 ? optionId(selectedKey) : undefined}
              className="group/records outline-none"
              onKeyDown={onListKeyDown}
              onFocus={onListFocus}
            >
              {records.map((record) => {
                const key = recordKey(record);
                const isSelected = key === selectedKey;
                return (
                  <div
                    key={key}
                    id={optionId(key)}
                    role="option"
                    aria-selected={isSelected}
                    data-record-key={key}
                    className={`cursor-default rounded-md px-2 py-1.5 transition-colors ${
                      isSelected
                        ? "bg-primary-surface group-focus-visible/records:ring-2 group-focus-visible/records:ring-inset group-focus-visible/records:ring-focus-ring"
                        : "hover:bg-surface-muted"
                    }`}
                    onMouseDown={(event) => {
                      event.preventDefault();
                      select(record);
                      listRef.current?.focus();
                    }}
                  >
                    <div className="flex min-w-0 items-center gap-2 text-xs text-ink-muted">
                      <span className="tabular-nums">{formatTime(rowTime, record.time)}</span>
                      <LevelPill level={record.level} />
                      {record.kind === "log" ? null : <KindPill kind={record.kind} />}
                    </div>
                    <div data-record-title className="truncate text-sm text-ink-strong">{record.title}</div>
                    {summaryText(record, text) ? (
                      <div className="truncate text-xs text-ink-muted">{summaryText(record, text)}</div>
                    ) : null}
                  </div>
                );
              })}
            </div>
          )}
          {list.status === "ready" && list.loadingMore ? (
            <p className="px-2 py-3 text-sm text-ink-muted">{t("records.loading")}</p>
          ) : null}
          {list.status === "ready" && list.moreFailed ? (
            <OperationResult level="error" className="m-2">{t("records.loadFailed")}</OperationResult>
          ) : null}
        </div>
      </section>
      <div
        role="separator"
        aria-orientation="vertical"
        aria-label={t("records.resizeList")}
        aria-controls="records-list-pane"
        aria-valuemin={RECORDS_LIST_WIDTH.min}
        aria-valuemax={RECORDS_LIST_WIDTH.max}
        aria-valuenow={shownListWidth}
        tabIndex={0}
        style={{ width: SPLITTER_WIDTH }}
        className="group flex shrink-0 cursor-col-resize justify-center outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-focus-ring"
        onMouseDown={beginListDrag}
        onKeyDown={resizeByKey}
        onKeyUp={commitKeyedWidth}
        onBlur={commitKeyedWidth}
      >
        <div className="w-px bg-border transition-colors group-hover:bg-border-strong" />
      </div>
      <section className="flex min-w-0 flex-1 flex-col overflow-hidden" aria-busy={detail.status === "loading"}>
        {detail.status === "ready" ? (
          <RecordDetailView record={detail.record} launchLabel={launchLabel} />
        ) : detail.status === "failed" ? (
          <OperationResult level="error" className="m-4">{t("records.detailFailed")}</OperationResult>
        ) : detail.status === "none" ? (
          <p className="m-auto px-4 text-center text-sm text-ink-muted">{t("records.noSelection")}</p>
        ) : null}
      </section>
    </div>
  );
}

// A row's second line: the sentence an Issue or a notification showed, else
// what the record says beside its title.
function summaryText(record: RecordSummary, text: (message: Message) => string): string | null {
  const sentence = recordSentence(record);
  return sentence === null ? record.text : text(sentence);
}

function LevelPill({ level }: { level: RecordLevel }) {
  const { t } = useI18n();
  return (
    <span className={`shrink-0 rounded-full px-1.5 text-xs leading-5 ${LEVEL_TONES[level]}`}>
      {t(LEVEL_LABELS[level])}
    </span>
  );
}

function KindPill({ kind }: { kind: RecordKind }) {
  const { t } = useI18n();
  return (
    <span className="min-w-0 truncate rounded-full bg-surface-muted px-1.5 text-xs leading-5 text-ink-muted">
      {t(KIND_LABELS[kind])}
    </span>
  );
}

function FilterSelect({
  label,
  value,
  allLabel,
  options,
  onChange,
}: {
  label: string;
  value: string | null;
  allLabel: string;
  options: { value: string; label: string }[];
  onChange: (value: string | null) => void;
}) {
  // A chosen value the sources no longer list stays selectable until changed.
  const shown =
    value === null || options.some((option) => option.value === value)
      ? options
      : [{ value, label: value }, ...options];
  return (
    <span className="flex min-w-0 flex-1 [&>span]:w-full">
      <Select
        aria-label={label}
        value={value ?? ""}
        className="w-full"
        onChange={(event) => onChange(event.target.value === "" ? null : event.target.value)}
      >
        <option value="">{allLabel}</option>
        {shown.map((option) => (
          <option key={option.value} value={option.value}>
            {option.label}
          </option>
        ))}
      </Select>
    </span>
  );
}

function RecordDetailView({
  record,
  launchLabel,
}: {
  record: RecordDetail;
  launchLabel: (session: string) => string;
}) {
  const { t, text, locale } = useI18n();
  const zone = useDisplayZone();
  const exactTime = useMemo(
    () =>
      new Intl.DateTimeFormat(locale, {
        year: "numeric",
        month: "2-digit",
        day: "2-digit",
        hour: "2-digit",
        minute: "2-digit",
        second: "2-digit",
        fractionalSecondDigits: 3,
      }),
    // The zone is read by the formatter when it is made.
    [locale, zone],
  );

  const fields: { label: ReactNode; key: string; value: ReactNode }[] = [];
  const blocks: { label: string; key: string; text: string }[] = [];
  for (const field of record.fields) {
    if (field.value === null || field.value === "") continue;
    const value = String(field.value);
    const presentation = fieldPresentation(record.kind, field.name);
    if (presentation === null) {
      fields.push({ key: field.name, label: <code className="font-mono">{field.name}</code>, value: <code className="break-all font-mono">{value}</code> });
      continue;
    }
    const label = t(presentation.label);
    switch (presentation.shape) {
      case "json":
        blocks.push({ key: field.name, label, text: prettyJson(value) });
        break;
      case "time":
        fields.push({ key: field.name, label, value: formatTime(exactTime, value) });
        break;
      case "launch":
        fields.push({ key: field.name, label, value: launchLabel(value) });
        break;
      case "milliseconds":
        fields.push({ key: field.name, label, value: t("activity.durationMs", { count: Number(field.value) }) });
        break;
      case "text":
        fields.push({ key: field.name, label, value: <span className="break-words">{value}</span> });
        break;
      case "sentence": {
        const sentence = recordSentence(record);
        fields.push({ key: field.name, label, value: <span className="break-words">{sentence === null ? value : text(sentence)}</span> });
        break;
      }
      case "value":
        fields.push({ key: field.name, label, value: <code className="break-all font-mono">{value}</code> });
        break;
    }
  }

  return (
    <>
      <div className="flex shrink-0 items-start justify-between gap-3 border-b border-border px-4 py-3">
        <h2 className="min-w-0 select-text break-words text-sm font-semibold text-ink-strong">{record.title}</h2>
        <div className="flex shrink-0 items-center gap-2 pt-px">
          <LevelPill level={record.level} />
          <KindPill kind={record.kind} />
        </div>
      </div>
      <div
        data-records-detail-body
        role="region"
        tabIndex={0}
        aria-label={t("records.details")}
        className="relative min-h-0 flex-1 select-text overflow-y-auto px-4 py-3 text-sm"
      >
        <dl className="grid grid-cols-[fit-content(40%)_minmax(0,1fr)] gap-x-4 gap-y-1.5">
          {fields.map((field) => (
            <div key={field.key} className="contents">
              <dt className="break-words text-ink-muted">{field.label}</dt>
              <dd className="min-w-0 text-ink">{field.value}</dd>
            </div>
          ))}
        </dl>
        {blocks.map((block) => (
          <section key={block.key} data-records-block className="mt-4">
            <h3 className="mb-1 text-xs font-semibold text-ink-muted">{block.label}</h3>
            <pre className="whitespace-pre-wrap break-words rounded-lg border border-border bg-surface p-2 font-mono text-xs text-ink">
              {block.text}
            </pre>
          </section>
        ))}
      </div>
    </>
  );
}
