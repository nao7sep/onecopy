import { memo } from "react";
import { useI18n } from "../i18n/I18nContext";
import type { TranscriptSegment } from "../models/transcript";
import { usePlaybackClientStore } from "../state/playback-client-store";
import { requestPlaybackSeek } from "../workflows/playback";
import { LinkButton } from "./ui/Button";

const TranscriptRow = memo(function TranscriptRow({ hash, segment, nextSeconds }: {
  hash: string;
  segment: TranscriptSegment;
  nextSeconds: number | undefined;
}) {
  const { t } = useI18n();
  // Playback ticks only rerender the rows whose highlighted state changes.
  // The transcript and its containing controls do not subscribe to the clock.
  const current = usePlaybackClientStore((state) => {
    const playback = state.session;
    return playback?.key === hash && playback.position >= segment.seconds &&
      (nextSeconds === undefined || playback.position < nextSeconds);
  });
  return <li className={`flex items-baseline gap-2 rounded px-1 py-0.5 ${current ? "bg-primary-surface" : ""}`}>
    {segment.timestamp !== "" ? <LinkButton
      tone="primary"
      className="shrink-0 font-mono text-xs"
      title={t("preview.playFrom", { time: segment.timestamp })}
      onClick={() => requestPlaybackSeek(hash, segment.seconds)}
    >{segment.timestamp}</LinkButton> : null}
    <span className="min-w-0 whitespace-pre-wrap break-words">{segment.text}</span>
  </li>;
});

export default memo(function TranscriptRows({ hash, segments }: {
  hash: string;
  segments: readonly TranscriptSegment[];
}) {
  return segments.map((segment, index) => <TranscriptRow
    key={`${segment.seconds}-${index}`}
    hash={hash}
    segment={segment}
    nextSeconds={segments[index + 1]?.seconds}
  />);
});
