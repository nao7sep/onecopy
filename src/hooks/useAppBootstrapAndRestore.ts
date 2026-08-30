// One-shot application startup and persisted-session restoration. Store
// loading remains in the bootstrap workflow; this hook owns the point where
// loaded state is projected into shell-local and feature-owned state.

import { useCallback, useEffect, useRef, useState } from "react";
import type { LoadedAppData } from "../repositories";
import type { SectionCounts } from "../models/sections";
import { DEFAULT_DESC, type SortChoice, type SortOrder } from "../models/items";
import { useItemsStore } from "../state/items-store";
import { useAppStore } from "../state/app-store";
import { usePreviewStore } from "../state/preview-store";
import { bootstrapApplication } from "../workflows/app-lifecycle";
import { parseAnchorContext } from "../models/mainSelection";

interface AppBootstrapAndRestoreOptions {
  appData: LoadedAppData | null;
  counts: SectionCounts | null;
  restorePaneIntents: (state: Record<string, unknown>) => void;
}

export function useAppBootstrapAndRestore({
  appData,
  counts,
  restorePaneIntents,
}: AppBootstrapAndRestoreOptions) {
  const [rightTab, setRightTabRaw] = useState<"details" | "destinations">("details");
  const setRightTab = useCallback((tab: "details" | "destinations") => {
    setRightTabRaw(tab);
    void useAppStore.getState().patchState({ rightPaneTab: tab });
  }, []);

  useEffect(() => {
    void bootstrapApplication();
  }, []);

  const restoredRef = useRef(false);
  useEffect(() => {
    if (restoredRef.current || appData === null || counts === null) return;
    restoredRef.current = true;
    const state = appData.state ?? {};
    const isOrder = (value: unknown): value is SortOrder =>
      value === "time" ||
      value === "name" ||
      value === "size" ||
      value === "resolution" ||
      value === "ext";
    const asChoice = (value: unknown): SortChoice | null => {
      if (isOrder(value)) return { order: value, desc: DEFAULT_DESC[value] };
      if (typeof value !== "object" || value === null) return null;
      const record = value as Record<string, unknown>;
      return isOrder(record.order)
        ? { order: record.order, desc: record.desc === true }
        : null;
    };

    const savedOrders = (state.sortOrders ?? {}) as Record<string, unknown>;
    useItemsStore.setState((current) => ({
      sortOrders: {
        media:
          asChoice(savedOrders.media) ?? asChoice(state.sortOrder) ?? current.sortOrders.media,
        other: asChoice(savedOrders.other) ?? current.sortOrders.other,
      },
    }));
    if (state.rightPaneTab === "destinations") setRightTabRaw("destinations");
    restorePaneIntents(state);

    const placement = state.previewPlacement;
    usePreviewStore
      .getState()
      .restoreFollow(
        state.previewFollow === true,
        placement === "split" || placement === "window" ? placement : null,
      );

    const last = state.lastSection as { kind?: string; month?: string } | undefined;
    if (
      last === undefined ||
      (last.kind !== "image" && last.kind !== "video" && last.kind !== "other") ||
      typeof last.month !== "string"
    ) {
      return;
    }
    const lists =
      last.kind === "image" ? counts.images : last.kind === "video" ? counts.videos : counts.others;
    if (!lists.some((section) => section.month === last.month)) return;
    const anchor = typeof state.lastItem === "string" ? state.lastItem : null;
    void useItemsStore.getState().select(
      { kind: last.kind, month: last.month },
      { anchor, context: parseAnchorContext(state.lastItemContext) },
    );
  }, [appData, counts, restorePaneIntents]);

  return { rightTab, setRightTab };
}
