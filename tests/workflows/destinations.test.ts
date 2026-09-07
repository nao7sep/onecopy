// @vitest-environment happy-dom

import { beforeEach, describe, expect, it } from "vitest";
import {
  addDestinationRoot,
  removeDestinationRoot,
} from "../../src/workflows/destinations";
import { useDestinationsStore } from "../../src/state/destinations-store";
import {
  invokeCalls,
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
    expect(invokeCalls.find((call) => call.command === "patch_config")?.args).toMatchObject({
      reportFailure: false,
    });
  });

  it("serializes root edits so a delayed save cannot overwrite a later intent", async () => {
    let finishFirst: (() => void) | undefined;
    let saves = 0;
    mockCommands({
      patch_config: () => {
        saves += 1;
        if (saves === 1) {
          return new Promise<Record<string, never>>((resolve) => {
            finishFirst = () => resolve({});
          });
        }
        return {};
      },
    });
    openDialog.mockResolvedValueOnce("/added");

    const add = addDestinationRoot();
    await Promise.resolve();
    const remove = removeDestinationRoot("/existing");
    await Promise.resolve();
    expect(saves).toBe(1);

    finishFirst?.();
    await Promise.all([add, remove]);

    expect(useDestinationsStore.getState().roots).toEqual(["/added"]);
    expect(
      invokeCalls.filter((call) => call.command === "patch_config").map((call) =>
        call.args.patch,
      ),
    ).toEqual([
      { destinationRoots: ["/existing", "/added"] },
      { destinationRoots: ["/added"] },
    ]);
  });
});
