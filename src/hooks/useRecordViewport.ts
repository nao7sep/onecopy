// Measured virtual rows for the bounded Records history window. One geometry
// owner supplies rendering, keyboard reveal, and anchors through page eviction.
import { useCallback, useLayoutEffect, useRef, useState, type RefObject } from "react";
import { recordKey, type RecordSummary } from "../models/records";
import { variableWindow } from "../utils/virtualize";

const ESTIMATED_HEIGHT = 72;

export function useRecordViewport(records: readonly RecordSummary[], scrollRef: RefObject<HTMLDivElement | null>, listRef: RefObject<HTMLDivElement | null>) {
  const heights = useRef(new Map<string, number>());
  const [revision, setRevision] = useState(0);
  const [viewport, setViewport] = useState({ top: 0, height: 600 });
  const anchor = useRef<{ key: string; delta: number } | null>(null);
  const win = variableWindow(records.map((record) => heights.current.get(recordKey(record)) ?? ESTIMATED_HEIGHT), viewport.top, viewport.height);
  const geometry = useRef({ records, offsets: win.offsets });
  geometry.current = { records, offsets: win.offsets };

  const onScroll = useCallback(() => {
    const scroll = scrollRef.current;
    if (!scroll) return;
    const top = Math.max(0, scroll.scrollTop - (listRef.current?.offsetTop ?? 0));
    const height = scroll.clientHeight;
    setViewport((old) => old.top === top && old.height === height ? old : { top, height });
  }, [scrollRef, listRef]);

  const preserveAnchor = useCallback(() => {
    if (anchor.current) return;
    const scroll = scrollRef.current;
    const { records: rows, offsets } = geometry.current;
    if (!scroll || rows.length === 0) return;
    const top = Math.max(0, scroll.scrollTop - (listRef.current?.offsetTop ?? 0));
    let index = 0;
    while (index < rows.length - 1 && offsets[index + 1]! <= top) index++;
    anchor.current = { key: recordKey(rows[index]!), delta: top - offsets[index]! };
  }, [scrollRef, listRef]);

  useLayoutEffect(() => {
    const pending = anchor.current;
    anchor.current = null;
    if (pending && scrollRef.current) {
      const index = records.findIndex((record) => recordKey(record) === pending.key);
      if (index >= 0) scrollRef.current.scrollTop = (listRef.current?.offsetTop ?? 0) + win.offsets[index]! + pending.delta;
    }
    onScroll();
  }, [records, revision, onScroll]);

  useLayoutEffect(() => {
    const retained = new Set(records.map(recordKey));
    for (const key of heights.current.keys()) if (!retained.has(key)) heights.current.delete(key);
    const nodes = Array.from(listRef.current?.querySelectorAll<HTMLElement>("[data-record-key]") ?? []);
    const measure = () => {
      let changed = false;
      for (const node of nodes) {
        const key = node.dataset.recordKey!;
        const height = node.getBoundingClientRect().height;
        if (height > 0 && height !== heights.current.get(key)) {
          if (!changed) preserveAnchor();
          heights.current.set(key, height);
          changed = true;
        }
      }
      if (changed) setRevision((value) => value + 1);
      onScroll();
    };
    measure();
    const observer = new ResizeObserver(measure);
    for (const node of nodes) observer.observe(node);
    if (scrollRef.current) observer.observe(scrollRef.current);
    return () => observer.disconnect();
  }, [records, win.startRow, win.endRow, preserveAnchor, onScroll]);

  const reveal = (key: string) => {
    const scroll = scrollRef.current;
    const index = records.findIndex((record) => recordKey(record) === key);
    if (!scroll || index < 0) return;
    const base = listRef.current?.offsetTop ?? 0;
    const start = base + win.offsets[index]!;
    const end = base + win.offsets[index + 1]!;
    if (start < scroll.scrollTop) scroll.scrollTop = start;
    else if (end > scroll.scrollTop + scroll.clientHeight) scroll.scrollTop = end - scroll.clientHeight;
    onScroll();
  };

  return { ...win, onScroll, preserveAnchor, reveal };
}
