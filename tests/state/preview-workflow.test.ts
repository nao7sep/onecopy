import { beforeEach, describe, expect, it } from "vitest";
import { installPreviewCommandWiring } from "../../src/workflows/preview";
import { listen, resetTauriMocks } from "../mocks/tauri";

beforeEach(() => resetTauriMocks());

describe("preview command wiring", () => {
  it("propagates a listener rejection and permits a clean retry", async () => {
    const cause = new Error("preview listener registration failed");
    listen.mockRejectedValueOnce(cause);

    await expect(installPreviewCommandWiring()).rejects.toBe(cause);
    await expect(installPreviewCommandWiring()).resolves.toBeUndefined();

    expect(listen).toHaveBeenCalledTimes(2);
  });
});
