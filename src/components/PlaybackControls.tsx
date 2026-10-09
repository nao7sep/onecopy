import { useRef, useState } from "react";
import { useI18n } from "../i18n/I18nContext";
import { useAppStore } from "../state/app-store";
import { configFlag } from "../models/config";
import { clampPlaybackVolume, volumeIconLevel } from "../models/playback";
import { setSoundEnabled, setAutoplay, setPlaybackVolume } from "../workflows/playback";
import { reportActionFailure } from "../state/notifications-store";
import { message } from "../i18n/translate";
import Button from "./ui/Button";

export default function PlaybackControls() {
  const { t, number } = useI18n();
  const config = useAppStore((s) => s.appData?.config);
  const audible = configFlag(config, "soundEnabled");
  const autoplay = configFlag(config, "autoplay");
  const volume = clampPlaybackVolume(config?.playbackVolume);
  const level = volumeIconLevel(audible, volume);
  const volumePercent = audible ? Math.round(volume * 100) : 0;
  const busyRef = useRef(false);
  const [busy, setBusy] = useState(false);
  const change = async (action: "sound" | "autoplay") => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    try {
      if (action === "sound") await setSoundEnabled(!audible);
      else await setAutoplay(!autoplay);
    } catch (error) {
      reportActionFailure(`${action}-setting-failed`, message(action === "sound" ? "settings.saveFailed" : "app.autoplayChangeFailed"), error);
    } finally { busyRef.current = false; setBusy(false); }
  };
  return <span className="inline-flex shrink-0 items-center gap-2">
    <Button size="xs" variant="ghost" disabled={busy} aria-pressed={autoplay}
      title={t("settings.autoplay")} onClick={() => void change("autoplay")}
      className={autoplay ? "text-primary" : undefined}>
      {t(autoplay ? "app.autoplayOn" : "app.autoplayOff")}
    </Button>
    <span className="inline-flex items-center gap-1">
      <Button size="xs" variant="ghost" disabled={busy} aria-pressed={!audible}
        aria-label={t("app.soundToggle")} title={t(audible ? "app.soundOn" : "app.soundOff")}
        className="w-8 px-0!" onClick={() => void change("sound")}>
        <svg className="shrink-0" aria-hidden="true" data-volume-level={level} width="22" height="20" viewBox="0 0 28 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round">
          <path d="M11 5 6 9H3v6h3l5 4V5Z" />
          {level === 0 ? <path d="m17 9 6 6m0-6-6 6" /> : <>
            <path d="M15 9a5 5 0 0 1 0 6" />
            {level >= 2 ? <path d="M19 6a9 9 0 0 1 0 12" /> : null}
            {level >= 3 ? <path d="M23 3a13 13 0 0 1 0 18" /> : null}
          </>}
        </svg>
      </Button>
      <input type="range" min={0} max={100} step={1} value={volumePercent}
        disabled={busy} aria-label={t("settings.playbackVolume")} aria-valuetext={`${number(volumePercent)}%`}
        className="h-6 w-20 cursor-pointer accent-primary disabled:opacity-50"
        onChange={(event) => setPlaybackVolume(Number(event.target.value) / 100)} />
    </span>
  </span>;
}
