// The preview's show/hide and placement control.
//
// It lives in the app chrome rather than in Settings (the developer's call):
// placement is something you change while looking at photos — "put it on the
// other screen", "get it out of my way" — not a preference you go and
// configure. Before this the rule was implicit (second monitor if one exists)
// and the only control was an undiscoverable `P`, which made running OneCopy
// on ONE screen of several impossible to ask for.
//
// Both placement buttons show ALWAYS — monitor counting left the preview
// path entirely. Splitting one screen into two windows is a legitimate
// choice, so the pair is never gated on hardware.

import { Columns2, Eye, EyeOff, Monitor } from "lucide-react";
import { resolvePlacement, usePreviewStore } from "../state/preview-store";

export default function PreviewControl() {
  const follow = usePreviewStore((s) => s.follow);
  const preference = usePreviewStore((s) => s.placementPreference);
  const toggleFollow = usePreviewStore((s) => s.toggleFollow);
  const setPlacementPreference = usePreviewStore((s) => s.setPlacementPreference);

  const effective = resolvePlacement(preference);

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
    </span>
  );
}
