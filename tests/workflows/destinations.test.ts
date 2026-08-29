// @vitest-environment happy-dom

import { beforeEach, describe, expect, it } from "vitest";
import {
  addDestinationRoot,
  removeDestinationRoot,
} from "../../src/workflows/destinations";
import { useDestinationsStore } from "../../src/state/destinations-store";
import {
  mockCommands,
  openDialog,
  resetTauriMocks,
} from "../mocks/tauri";

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  mockCommands({ patch_config: () => ({}) });
  useDestinationsStore.setState({ roots: ["/existing"], message: "" });
});

describe("destination root failures", () => {
  it("keeps a failed directory picker visible", async () => {
    openDialog.mockRejectedValueOnce(new Error("picker unavailable"));

    await addDestinationRoot();

    expect(useDestinationsStore.getState()).toMatchObject({
      roots: ["/existing"],
      message: "Couldn’t add that destination.",
    });
  });

  it("keeps a failed config update visible without changing the tree", async () => {
    mockCommands({ patch_config: () => Promise.reject(new Error("disk full")) });

    await removeDestinationRoot("/existing");

    expect(useDestinationsStore.getState()).toMatchObject({
      roots: ["/existing"],
      message: "Couldn’t remove that destination.",
    });
  });
});
