// @vitest-environment happy-dom
// One isolated app-start scenario: the installer intentionally lives for the
// module's lifetime, so prior successful Preview use must not pre-install it.
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it } from "vitest";
import PreviewSurface from "../../src/components/PreviewSurface";
import { useTranscriptStore } from "../../src/state/transcript-store";
import { useContentSessionStore } from "../../src/state/content-session-store";
import { listen, mockCommands, resetTauriMocks } from "../mocks/tauri";

const AUDIO_DETAIL = {
  fileName: "interview.m4a", kind: "audio", byteSize: 1000,
  width: null, height: null, durationMs: 30000, stripFrames: null,
  dateState: "dated" as const, resolvedUtcMs: 0, resolvedSource: "metadata", dateOnly: false,
  copyPaths: ["/fixture/interview.m4a"], companionPaths: [],
};
const OTHER_DETAIL = { ...AUDIO_DETAIL, fileName: "notes.txt", kind: "other", durationMs: null };

beforeEach(() => {
  resetTauriMocks();
  mockCommands({
    transcript_get: () => ({ status: "ready", text: "[0:01] hello", message: null }),
    text_preview: () => ({
      body: "text", text: "first line", encoding: "utf-8", contentKey: "text-hash",
      encodings: ["utf-8"], byteSize: 10,
    }),
    log_event: () => null,
  });
  useTranscriptStore.setState({ rows: {} });
  useContentSessionStore.setState({
    textWrap: true, textEncodings: {}, transcriptViews: {},
    transcriptOpen: { video: false, audio: true },
  });
});
afterEach(cleanup);

it("retains a failed shared-session listener at each visible owner and retries from a control", async () => {
  mockCommands({ record_recent_notification: () => ({}) });
  listen.mockRejectedValueOnce(
    new Error("TypeError: EACCES /private/tmp/content listener IPC sentinel"),
  );

  render(
    <>
      <PreviewSurface surface="quick" hash={null} pathId={8} detail={OTHER_DETAIL} />
      <PreviewSurface surface="preview-split" hash="audio-hash" detail={AUDIO_DETAIL} />
    </>,
  );

  expect(
    await screen.findByText(
      "Preview settings could not be synchronized. Try a preview control again.",
    ),
  ).toBeTruthy();
  expect(
    await screen.findByText(
      "Transcript view settings could not be synchronized. Try Expand or Collapse again.",
    ),
  ).toBeTruthy();
  expect(document.body.textContent).not.toContain("EACCES");
  expect(document.body.textContent).not.toContain("/private/tmp");
  expect(document.body.textContent).not.toContain("IPC sentinel");

  fireEvent.click(await screen.findByRole("button", { name: "Wrap on" }));
  await act(async () => {});

  expect(
    screen.queryByText(
      "Preview settings could not be synchronized. Try a preview control again.",
    ),
  ).toBeNull();
  expect(
    screen.getByText(
      "Transcript view settings could not be synchronized. Try Expand or Collapse again.",
    ),
  ).toBeTruthy();
  expect(
    listen.mock.calls.filter(([event]) => event === "content-session://state"),
  ).toHaveLength(2);
});
