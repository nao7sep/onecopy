import { useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { log, toErrorFields } from "../repositories";
import { recordActionFailure } from "../state/notifications-store";

const TIMEZONE_HELP_URL =
  "https://en.wikipedia.org/wiki/List_of_tz_database_time_zones";

export default function TimezoneHelpLink() {
  const [failed, setFailed] = useState(false);

  const openHelp = async () => {
    try {
      await openUrl(TIMEZONE_HELP_URL);
      setFailed(false);
    } catch (error) {
      setFailed(true);
      log.warn("timezone help link failed", toErrorFields(error));
      recordActionFailure(
        "timezone-help-link-failed",
        "Couldn’t open the timezone-name reference.",
        error,
      );
    }
  };

  return (
    <span className="block text-xs text-ink-muted">
      Use an IANA name, such as Asia/Tokyo.{" "}
      <button
        type="button"
        className="rounded-sm text-primary underline decoration-primary/50 underline-offset-2 hover:text-primary-hover focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary-ring"
        onClick={() => void openHelp()}
      >
        View timezone names
      </button>
      {failed ? " — couldn’t open the reference" : ""}
    </span>
  );
}
