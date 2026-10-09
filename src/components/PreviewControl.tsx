// The preview's show/hide and placement control.
//
// It lives in the app chrome rather than in Settings (the developer's call):
// placement is something you change while looking at photos — "put it in its
// own window", "get it out of my way" — not a preference you go and
// configure. Before this the rule was implicit (second monitor if one exists)
// and the only control was an undiscoverable `P`, which made running OneCopy
// on ONE screen of several impossible to ask for.
//
// Both placement buttons show ALWAYS — monitor counting left the preview
// path entirely. Splitting one screen into two windows is a legitimate
// choice, so the pair is never gated on hardware.

import { Columns2, Eye, EyeOff, Monitor } from "lucide-react";
import { useI18n } from "../i18n/I18nContext";
import { resolvePlacement, usePreviewStore } from "../state/preview-store";
import { setPreviewPlacement, togglePreview } from "../workflows/preview";
import Button, { IconButton } from "./ui/Button";

export default function PreviewControl() {
  const { t } = useI18n();
  const follow = usePreviewStore((s) => s.follow);
  const preference = usePreviewStore((s) => s.placementPreference);

  const effective = resolvePlacement(preference);

  return (
    <span className="flex items-center gap-2">
      <Button
        size="toolbar"
        variant="ghost"
        selected={follow}
        aria-pressed={follow}
        title={follow ? t("preview.hide") : t("preview.show")}
        onClick={() => void togglePreview()}
      >
        {follow ? <Eye size={14} /> : <EyeOff size={14} />}
        {t("preview.label")}
      </Button>
      {/* One segmented control: a bordered track whose chosen segment takes
          the selection fill. */}
      <span className="flex h-7 items-center gap-0.5 rounded-md border border-control-edge p-px">
        <IconButton
          size="sm"
          selected={effective === "split"}
          aria-pressed={effective === "split"}
          title={t("preview.placeInWindow")}
          className="rounded-[5px]"
          onClick={() => void setPreviewPlacement("split")}
        >
          <Columns2 size={13} />
        </IconButton>
        <IconButton
          size="sm"
          selected={effective === "window"}
          aria-pressed={effective === "window"}
          title={t("preview.placeSeparateWindow")}
          className="rounded-[5px]"
          onClick={() => void setPreviewPlacement("window")}
        >
          <Monitor size={13} />
        </IconButton>
      </span>
    </span>
  );
}
