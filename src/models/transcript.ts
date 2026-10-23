export interface TranscriptSegment {
  seconds: number;
  timestamp: string;
  text: string;
}

export function parseTranscript(text: string): TranscriptSegment[] {
  return text
    .split(/\r?\n/)
    .filter((line) => line.trim() !== "")
    .map((line) => {
      const match = line.match(/^\[(\d+):(\d{2})\]\s?(.*)$/);
      if (match === null) return { seconds: 0, timestamp: "", text: line };
      return {
        seconds: Number(match[1]) * 60 + Number(match[2]),
        timestamp: `${match[1]}:${match[2]}`,
        text: match[3],
      };
    });
}
