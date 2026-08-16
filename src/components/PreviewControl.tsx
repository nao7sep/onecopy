// The preview's show/hide and placement control.
//
// It lives in the app chrome rather than in Settings (the developer's call):
// placement is something you change while looking at photos — "put it on the
// other screen", "get it out of my way" — not a preference you go and
// configure. Before this the rule was implicit (second monitor if one exists)
// and the only control was an undiscoverable `P`, which made running OneCopy
// on ONE screen of several impossible to ask for.
//
// The placement pair only appears with two or more monitors: on a single
// screen there is nothing to choose between, and a disabled control that never
// becomes enabled is just clutter.

import { Columns2, Eye, EyeOff, Monitor } from "lucide-react";
import { usePreviewStore } from "../state/preview-store";

export default function PreviewControl() {
  const follow = usePreviewStore((s) => s.follow);
  const preference = usePreviewStore((s) => s.placementPreference);
  const screenCount = usePreviewStore((s) => s.screenCount);
  const toggleFollow = usePreviewStore((s) => s.toggleFollow);
  const setPlacementPreference = usePreviewStore((s) => s.setPlacementPreference);

  // Auto resolves to the second screen when one exists, so that is what the
  // pair should show as chosen — the control must never claim a placement the
  // preview would not actually use.
  const effective = preference ?? (screenCount >= 2 ? "window" : "split");

  return (
    <span className="flex items-center gap-1">
      <button
        aria-pressed={follow}
        title={follow ? "Hide preview (Space)" : "Show preview (Space)"}
        className={`flex h-7 items-center gap-1.5 rounded-md px-2 text-xs transition-colors ${
          follow
            ? "bg-primary-surface text-primary"
            : "text-ink-muted hover:bg-surface-muted hover:text-ink"
        }`}
        onClick={() => void toggleFollow()}
      >
        {follow ? <Eye size={14} /> : <EyeOff size={14} />}
        Preview
      </button>
      {screenCount >= 2 ? (
        <span className="flex items-center rounded-md border border-border p-0.5">
          <button
            aria-pressed={effective === "split"}
            title="Show the preview inside this window"
            className={`flex h-6 w-6 items-center justify-center rounded transition-colors ${
              effective === "split"
                ? "bg-primary-surface text-primary"
                : "text-ink-muted hover:text-ink"
            }`}
            onClick={() => void setPlacementPreference("split")}
          >
            <Columns2 size={13} />
          </button>
          <button
            aria-pressed={effective === "window"}
            title="Show the preview on the second screen"
            className={`flex h-6 w-6 items-center justify-center rounded transition-colors ${
              effective === "window"
                ? "bg-primary-surface text-primary"
                : "text-ink-muted hover:text-ink"
            }`}
            onClick={() => void setPlacementPreference("window")}
          >
            <Monitor size={13} />
          </button>
        </span>
      ) : null}
    </span>
  );
}
