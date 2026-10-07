import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import { I18nProvider } from "../../src/i18n/I18nContext";
import { loadCatalogue } from "../../src/i18n/catalogues";
import type { ItemDetail } from "../../src/models/items";
import "../../src/App.css";
const offline = "/Volumes/Travel Archive/Photos";
const issue = {
  id: 1, path: offline, kind: "source-unavailable", message: "",
  messageKey: "source.driveUnavailable", messageValues: null,
  firstSeenUtc: "2026-10-07T00:00:00Z", lastSeenUtc: "2026-10-07T00:00:00Z", occurrenceCount: 1,
};
mockIPC((command) => {
  if (command === "plugin:event|listen") return 1;
  if (command === "get_issues") return { total: 1, rows: [issue] };
  if (["plugin:event|unlisten", "log_event", "activity_record"].includes(command)) return;
  throw new Error(`Native command blocked in visual fixture: ${command}`);
});
const { MissingSourcesNotice } = await import("../../src/components/SourceAvailability");
const { default: MetadataPane } = await import("../../src/components/MetadataPane");
const { default: IssuesModal } = await import("../../src/components/IssuesModal");
const { useWizardStore } = await import("../../src/state/wizard-store");
const params = new URLSearchParams(location.search);
const language = params.get("language") === "ja" ? "ja" : "en";
await loadCatalogue(language);
useWizardStore.setState({ missingDirs: [offline] });
const detail: ItemDetail = {
  fileName: "Travel notes.txt", kind: "other", byteSize: 1304,
  width: null, height: null, durationMs: null, stripFrames: null,
  dateState: "dated", resolvedUtcMs: Date.UTC(2026, 9, 6), resolvedSource: "filesystem", dateOnly: false,
  copyPaths: [`${offline}/Travel notes.txt`, "/Users/example/Pictures/Travel notes.txt"], companionPaths: [],
};
document.body.className = "bg-background text-ink";
createRoot(document.getElementById("root")!).render(<I18nProvider language={language} locale={language}>
  {params.get("surface") === "issues" ? <IssuesModal open onClose={() => {}} /> : <main>
    <MissingSourcesNotice missing={[offline]} onRecheck={() => {}} onReconfigure={() => {}} />
    <aside className="m-6 w-80 border border-border bg-surface"><MetadataPane detail={detail} hash={null} item={null} /></aside>
  </main>}
</I18nProvider>);
