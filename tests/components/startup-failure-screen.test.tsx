// @vitest-environment happy-dom
//
// The blocked launch: a required store a newer OneCopy wrote is named with
// its path and left in place, and any other failure keeps its one pair of
// sentences.

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { cleanup, render, fireEvent, screen, waitFor } from "@testing-library/react";
import StartupFailureScreen from "../../src/components/StartupFailureScreen";
import { useAppStore } from "../../src/state/app-store";
import { resetTauriMocks, mockCommands, invokeCalls } from "../mocks/tauri";
import { t } from "../helpers/i18n";

const failure = {
  title: "OneCopy could not start safely",
  message: "OneCopy could not safely open its application data.",
};

beforeEach(() => {
  resetTauriMocks();
});

afterEach(() => {
  cleanup();
  useAppStore.setState({ startupFailure: null });
});

describe("the startup failure screen", () => {
  it("names each store a newer OneCopy wrote, with its path", () => {
    useAppStore.setState({
      startupFailure: {
        ...failure,
        newerStores: [
          { file: "index.sqlite3", path: "/Users/x/.onecopy/index.sqlite3", version: 2, supported: 1 },
          { file: "config.json", path: "/Users/x/.onecopy/config.json", version: 2, supported: 1 },
        ],
      },
    });
    render(<StartupFailureScreen />);
    const text = document.body.textContent ?? "";
    expect(text).toContain(t("startup.newerBody"));
    expect(text).toContain("/Users/x/.onecopy/index.sqlite3");
    expect(text).toContain("config.json");
    expect(text).not.toContain(t("startup.blockedBody"));
  });

  it("keeps the general sentences for any other failure", () => {
    useAppStore.setState({ startupFailure: { ...failure, newerStores: [] } });
    render(<StartupFailureScreen />);
    const text = document.body.textContent ?? "";
    expect(text).toContain(t("startup.blockedBody"));
    expect(text).not.toContain(t("startup.newerBody"));
  });
});


it("renders native store names and paths with visible invalid-unit escapes", () => {
  const file = "config\u0000bff.json";
  const path = `/app/\u0000wd800/${file}`;
  useAppStore.setState({ startupFailure: { ...failure, newerStores: [{ file, path, version: 2, supported: 1 }] } });
  const view = render(<StartupFailureScreen />);
  expect(view.getByText("config\\xFF.json")).toBeTruthy();
  expect(view.getByText("/app/\\uD800/config\\xFF.json")).toBeTruthy();
  expect(useAppStore.getState().startupFailure?.newerStores[0]).toMatchObject({ file, path });
});


it("keeps full diagnostics behind Details and reveals the retained startup folder", async () => {
  mockCommands({ reveal_startup_data_folder: () => null });
  useAppStore.setState({ startupFailure: {
    ...failure, newerStores: [], diagnostic: "EACCES /private/staging STARTUP_SENTINEL", dataRoot: "/app/data",
  } });
  render(<StartupFailureScreen />);
  expect(screen.queryByText(/STARTUP_SENTINEL/)).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: t("records.details") }));
  expect(screen.getByText("EACCES /private/staging STARTUP_SENTINEL")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: t("startup.revealDataFolder") }));
  await waitFor(() => expect(invokeCalls).toContainEqual({ command: "reveal_startup_data_folder", args: {} }));
  expect(invokeCalls.some((call) => call.command === "reveal_data_subdir")).toBe(false);
});

it("retains the diagnostic record when opening its folder fails", async () => {
  mockCommands({ reveal_startup_data_folder: () => { throw new Error("EIO /private/OPEN_SENTINEL"); }, log_event: () => null });
  useAppStore.setState({ startupFailure: { ...failure, newerStores: [], diagnostic: "startup record", dataRoot: "/app/data" } });
  render(<StartupFailureScreen />);
  fireEvent.click(screen.getByRole("button", { name: t("records.details") }));
  fireEvent.click(screen.getByRole("button", { name: t("startup.revealDataFolder") }));
  expect(await screen.findByText(t("startup.dataFolderFailed"))).toBeTruthy();
  expect(screen.getByText("startup record")).toBeTruthy();
  expect(screen.queryByText(/OPEN_SENTINEL/)).toBeNull();
});
