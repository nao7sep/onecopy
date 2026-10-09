// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import PlaybackControls from "../../src/components/PlaybackControls";
import { useAppStore } from "../../src/state/app-store";
import { flushConfigForShutdown } from "../../src/state/app-store";
import { mockCommands, resetTauriMocks, invokeCalls } from "../mocks/tauri";
import { seedAppConfig } from "../helpers/config";

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  seedAppConfig({ playbackVolume: 0.6 });
  mockCommands({ save_config: ({ changes }) => ({ ...useAppStore.getState().appData!.config, ...(changes as object) }), log_event: () => null });
});
afterEach(async () => { await flushConfigForShutdown(); cleanup(); });

describe("shared playback controls", () => {
  it("has one autoplay toggle and saves one choice for both media types", async () => {
    render(<PlaybackControls />);
    const autoplay = screen.getByRole("button", { name: "Autoplay on" });
    await act(async () => fireEvent.click(autoplay));
    expect(screen.getByRole("button", { name: "Autoplay off" })).toBeTruthy();
    expect(invokeCalls.find((call) => call.command === "save_config")?.args.changes).toEqual({ autoplay: false });
  });

  it("mutes at zero, restores the previous level, and draws all volume states", async () => {
    const view = render(<PlaybackControls />);
    const slider = screen.getByRole("slider") as HTMLInputElement;
    const speaker = screen.getByRole("button", { name: "Toggle sound for every OneCopy player" });
    expect(slider.value).toBe("60");
    for (const [volume, icon] of [[10, 1], [50, 2], [90, 3], [0, 0]]) {
      fireEvent.change(slider, { target: { value: String(volume) } });
      expect(view.container.querySelector("svg")?.getAttribute("data-volume-level")).toBe(String(icon));
    }
    expect(useAppStore.getState().appData?.config).toMatchObject({ soundEnabled: false, playbackVolume: 0.9 });
    await act(async () => fireEvent.click(speaker));
    expect(slider.value).toBe("90");
    await act(async () => fireEvent.click(speaker));
    expect(slider.value).toBe("0");
    await act(async () => fireEvent.click(speaker));
    expect(slider.value).toBe("90");
  });
});
