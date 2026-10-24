import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { ItemDetail } from "../models/items";
import { thumbUrl } from "../models/items";
import type { GroupMember } from "../state/comparison-store";
import { useItemsStore } from "../state/items-store";
import { useI18n } from "../i18n/I18nContext";
import { log, toErrorFields } from "../repositories";
import { displayNativePath } from "../utils/nativePath";
import { visibleWindow } from "../utils/virtualize";
import { gridNavigationTarget } from "../models/gridNavigation";
import { isComposingEvent } from "../hooks/useComposing";
import { requestComparisonFromMain } from "../workflows/comparison";
import Button from "./ui/Button";
import OperationResult from "./ui/OperationResult";

/** Detail publication is owned by the selected-anchor reader. Every fresh
 * snapshot rechecks membership even when that anchor's hash is unchanged. */
export default function SimilarPhotoStrip({ hash, detail }: { hash: string; detail: ItemDetail }) {
  const { t } = useI18n();
  const [snapshot, setSnapshot] = useState<{ hash: string; rows: GroupMember[]; failed: boolean } | null>(null);
  const pending = useRef<{ hash: string } | null>(null);
  const reading = useRef(false);
  useEffect(() => {
    const intent = { hash };
    pending.current = intent;
    // A detail burst replaces pending intent, but the admitted native read
    // owns its slot until settlement. Only the latest reply may render.
    const drain = async () => {
      if (reading.current) return;
      reading.current = true;
      try {
        while (pending.current !== null) {
          const request = pending.current;
          try {
            const rows = await invoke<GroupMember[]>("get_similar_group", { hash: request.hash });
            if (pending.current === request) setSnapshot({ hash: request.hash, rows, failed: false });
          } catch (failure) {
            log.error("similar group load failed", toErrorFields(failure));
            if (pending.current === request) setSnapshot((previous) => ({ hash: request.hash,
              rows: previous?.hash === request.hash ? previous.rows : [], failed: true }));
          }
          if (pending.current === request) pending.current = null;
        }
      } finally { reading.current = false; }
    };
    void drain();
    return () => { if (pending.current === intent) pending.current = null; };
  }, [hash, detail]);
  // Keep the same collection/focus owner when choosing another member of
  // this group; unrelated anchors never display the previous group's rows.
  const belongs = snapshot?.hash === hash || snapshot?.rows.some((row) => row.hash === hash);
  const members = belongs ? snapshot!.rows : [];
  const failed = snapshot?.hash === hash && snapshot.failed;
  if (members.length < 2) {
    if (failed) return <OperationResult level="error" className="mt-3">{t("metadata.similarLoadFailed")}</OperationResult>;
    return snapshot?.hash === hash ? null : <p className="mt-3 text-xs text-ink-muted">{t("common.loading")}</p>;
  }
  return <div className="mb-1 mt-3">
    <div className="flex flex-wrap items-center justify-between gap-x-2 gap-y-1">
      <dt className="min-w-0 break-words text-xs text-ink-muted">{t("metadata.similar", { count: members.length })}</dt>
      <Button size="toolbar" onClick={() => void requestComparisonFromMain()}>{t("metadata.compare")}</Button>
    </div>
    <Members members={members} selected={hash} />
    {failed ? <OperationResult level="error" className="mt-1">{t("metadata.similarLoadFailed")}</OperationResult> : null}
  </div>;
}

/** A horizontal, manual-activation listbox. Navigation uses Main's shared
 * positional rules; letter shortcuts remain owned by the command layer. */
function Members({ members, selected }: { members: GroupMember[]; selected: string }) {
  const { t } = useI18n();
  const id = useId();
  const viewport = useRef<HTMLElement | null>(null);
  const ruler = useRef<HTMLSpanElement | null>(null);
  const [geometry, setGeometry] = useState({ width: 0, step: 84 });
  const [scrollLeft, setScrollLeft] = useState(0);
  const [cursor, setCursor] = useState(selected);
  const [focused, setFocused] = useState(false);
  const previousPosition = useRef(0);
  const found = members.findIndex((member) => member.hash === cursor);
  const activeIndex = found >= 0 ? found : Math.min(previousPosition.current, members.length - 1);
  const active = members[activeIndex]!;
  useLayoutEffect(() => {
    previousPosition.current = activeIndex;
    if (cursor !== active.hash) setCursor(active.hash);
  }, [activeIndex, active.hash, cursor]);
  useLayoutEffect(() => {
    const element = viewport.current!;
    const measure = () => setGeometry({ width: element.clientWidth, step: ruler.current!.getBoundingClientRect().width || 84 });
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    observer.observe(ruler.current!);
    return () => observer.disconnect();
  }, []);
  useLayoutEffect(() => {
    const element = viewport.current!;
    const start = activeIndex * geometry.step;
    const end = start + geometry.step;
    const next = start < element.scrollLeft ? start
      : end > element.scrollLeft + geometry.width ? Math.max(0, end - geometry.width)
        : element.scrollLeft;
    element.scrollLeft = next;
    setScrollLeft(next);
  }, [activeIndex, geometry]);
  const window = visibleWindow(scrollLeft, geometry.width, geometry.step, members.length);
  const mounted = new Set(Array.from({ length: Math.max(0, window.endRow - window.startRow) }, (_, offset) => window.startRow + offset));
  mounted.add(activeIndex);
  const activate = (hash: string) => { void useItemsStore.getState().selectIdentity(hash); };
  return <dd
    ref={viewport}
    role="listbox"
    aria-label={t("metadata.similar", { count: members.length })}
    aria-orientation="horizontal"
    aria-activedescendant={`${id}-${active.hash}`}
    tabIndex={0}
    className="mt-1 overflow-x-auto pb-1 outline-none"
    onFocus={() => setFocused(true)}
    onBlur={() => setFocused(false)}
    onScroll={(event) => setScrollLeft(event.currentTarget.scrollLeft)}
    onKeyDown={(event) => {
      if (isComposingEvent(event) || event.altKey || event.ctrlKey || event.metaKey) return;
      const target = gridNavigationTarget({ key: event.key, layout: "tiles", columns: 1,
        viewportHeight: geometry.width, rowHeight: geometry.step, current: activeIndex, total: members.length });
      if (target !== null) {
        event.preventDefault(); event.stopPropagation(); setCursor(members[target]!.hash);
      } else if (event.key === "Enter" || event.key === " ") {
        event.preventDefault(); event.stopPropagation(); activate(active.hash);
      }
    }}
  >
    <div className="relative h-20" style={{ width: members.length * geometry.step }}>
      <span ref={ruler} aria-hidden className="invisible absolute h-0" style={{ width: "5.25rem" }} />
      {[...mounted].sort((a, b) => a - b).map((index) => {
        const member = members[index]!;
        return <div
          key={member.hash}
          id={`${id}-${member.hash}`}
          role="option"
          aria-selected={member.hash === selected}
          aria-posinset={index + 1}
          aria-setsize={members.length}
          title={displayNativePath(member.fileName)}
          className={`absolute h-20 w-20 cursor-pointer overflow-hidden rounded-md border transition-colors motion-reduce:transition-none ${member.hash === selected
            ? "border-primary-ring bg-primary-surface ring-1 ring-inset ring-primary-ring"
            : "border-border bg-background hover:border-border-strong hover:bg-surface-muted active:bg-surface-pressed"} ${focused && index === activeIndex ? "outline outline-2 -outline-offset-2 outline-primary-ring" : ""}`}
          style={{ left: index * geometry.step }}
          onClick={() => { viewport.current?.focus(); setCursor(member.hash); activate(member.hash); }}
        >
          <img src={thumbUrl(member.hash)} alt={displayNativePath(member.fileName)} loading="lazy" className="h-full w-full object-contain" />
        </div>;
      })}
    </div>
  </dd>;
}
