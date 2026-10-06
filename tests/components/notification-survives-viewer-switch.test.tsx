// @vitest-environment happy-dom

// A persistent notice lives in `notifications.rs`'s process-wide ACTIVE list,
// not in any one window's component state. The fullscreen view is a separate
// webview with its own fresh module graph, so it must hydrate the notice from
// `get_active_notifications` on mount rather than from anything Main already
// holds in memory. This file exists
// solely so `installNotificationWiring()`'s module-level singleton has never
// run before this test — a shared file would let an earlier test's `install()`
// answer for this one too.

import { render } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import NotificationHost from "../../src/components/NotificationHost";
import { mockCommands, resetTauriMocks } from "../mocks/tauri";

beforeEach(() => resetTauriMocks());

it("hands a freshly opened fullscreen window the same persistent notice Main already recorded", async () => {
  mockCommands({
    get_active_notifications: () => [
      {
        id: 7,
        kind: "open-failed",
        path: null,
        level: "error",
        presentation: "persistent",
        message: "Couldn’t open the selected file.",
        messageKey: null,
        messageValues: null,
        firstSeenUtc: "2026-08-31T00:00:00.000Z",
        lastSeenUtc: "2026-08-31T00:00:00.000Z",
        occurrenceCount: 1,
      },
    ],
    dismiss_notification: () => true,
  });

  // Mirrors ViewerWindow.tsx mounting its own `<NotificationHost ownsTimedDismissal={false} />`.
  render(<NotificationHost ownsTimedDismissal={false} />);

  await vi.waitFor(() =>
    expect(document.body.textContent).toContain("Couldn’t open the selected file."),
  );
});
