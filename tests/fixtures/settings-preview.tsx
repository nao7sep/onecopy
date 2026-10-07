import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import { I18nProvider } from "../../src/i18n/I18nContext";
import { loadCatalogue } from "../../src/i18n/catalogues";
import config from "./effective-config.json";
import "../../src/App.css";
mockIPC((command) => {
  if (command === "plugin:event|listen") return 1;
  if (command === "visibility_capabilities") return { hiddenAttributes: true, systemAttributes: true };
  if (command === "text_preview_options") return { encodings: ["utf-8"], maxAllowedBytes: 16777216 };
  if (command === "plugin:window|available_monitors") return [];
  if (command === "plugin:event|unlisten" || command === "log_event" || command === "activity_record") return;
  throw new Error(`Native command blocked in visual fixture: ${command}`);
});
const { default: SettingsModal } = await import("../../src/components/SettingsModal");
const { default: PlaybackControls } = await import("../../src/components/PlaybackControls");
const { useSettingsStore } = await import("../../src/state/settings-store");
const { useAppStore } = await import("../../src/state/app-store");
const params = new URLSearchParams(location.search);
const language = params.get("language") === "ja" ? "ja" : "en";
await loadCatalogue(language);
useSettingsStore.getState().beginEditing(config, [], config);
useAppStore.setState({ appData: { config, state: {}, dataRoot: "/synthetic", debugEnabled: false, quarantines: [] } });
document.body.className = "bg-background text-ink";
createRoot(document.getElementById("root")!).render(<I18nProvider language={language} locale={language}>
  {params.get("surface") === "playback" ? <footer className="flex h-10 items-center justify-end gap-3 border-t border-border bg-surface px-3 text-xs"><PlaybackControls /></footer> : <SettingsModal open onClose={() => {}} />}
</I18nProvider>);
