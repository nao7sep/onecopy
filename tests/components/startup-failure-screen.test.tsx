// @vitest-environment happy-dom
//
// The blocked launch: a required store a newer OneCopy wrote is named with
// its path and left in place, and any other failure keeps its one pair of
// sentences.

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { cleanup, render } from "@testing-library/react";
import StartupFailureScreen from "../../src/components/StartupFailureScreen";
import { useAppStore } from "../../src/state/app-store";
import { resetTauriMocks } from "../mocks/tauri";
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
