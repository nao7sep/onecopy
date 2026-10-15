// @vitest-environment happy-dom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { useDisplayZone } from "../../src/hooks/useDisplayZone";
import { applyDisplayZoneSetting, displayTimeZone, subscribeDisplayZone } from "../../src/repositories/display-zone";
import { createTranslator } from "../../src/i18n/translate";
import { formatLocalMinute } from "../../src/utils/displayTime";
import { installDisplayZoneReconciliation } from "../../src/workflows/display-zone";
import { useItemsStore } from "../../src/state/items-store";
import { useSectionsStore } from "../../src/state/sections-store";

let zone = "Asia/Tokyo";
let offset = -540;
const dispose: (() => void)[] = [];

beforeEach(() => {
  vi.useFakeTimers();
  zone = "Asia/Tokyo";
  offset = -540;
  const DateTimeFormat = Intl.DateTimeFormat;
  vi.spyOn(Intl, "DateTimeFormat").mockImplementation(function (...args) {
    const formatter = new DateTimeFormat(...args);
    const resolved = formatter.resolvedOptions();
    formatter.resolvedOptions = () => ({ ...resolved, timeZone: zone });
    return formatter;
  });
  vi.spyOn(Date.prototype, "getTimezoneOffset").mockImplementation(() => offset);
});

afterEach(() => {
  applyDisplayZoneSetting("system");
  cleanup();
  for (const stop of dispose.splice(0)) stop();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

it("shares one environment observer and ignores unchanged focus and timer checks", () => {
  const first = vi.fn();
  const second = vi.fn();
  const stopFirst = subscribeDisplayZone(first);
  const stopSecond = subscribeDisplayZone(second);
  dispose.push(stopFirst, stopSecond);
  expect(vi.getTimerCount()).toBe(1);
  fireEvent.focus(window);
  vi.advanceTimersByTime(120_000);
  expect(first).not.toHaveBeenCalled();
  zone = "America/Los_Angeles";
  offset = 420;
  fireEvent.focus(window);
  expect(first).toHaveBeenCalledTimes(1);
  expect(second).toHaveBeenCalledTimes(1);
  stopFirst();
  expect(vi.getTimerCount()).toBe(1);
  zone = "America/Vancouver"; // Same offset, different month-boundary rules.
  expect(Intl.DateTimeFormat().resolvedOptions().timeZone).toBe(zone);
  document.dispatchEvent(new window.Event("visibilitychange"));
  expect(first).toHaveBeenCalledTimes(1);
  expect(second).toHaveBeenCalledTimes(2);
  stopSecond();
  expect(vi.getTimerCount()).toBe(0);
  fireEvent.focus(window);
  expect(second).toHaveBeenCalledTimes(2);
});

it("refreshes visible labels at a clock transition without remounting or stealing focus", () => {
  const mounted = vi.fn();
  function View() {
    const current = useDisplayZone();
    return <div><input ref={mounted} defaultValue="keep reading" /><output>{current}</output></div>;
  }
  render(<View />);
  const field = screen.getByRole("textbox");
  field.focus();
  offset = -600;
  act(() => vi.advanceTimersByTime(60_000));
  expect(screen.getByText("Asia/Tokyo:-600")).toBeTruthy();
  expect(screen.getByRole("textbox")).toBe(field);
  expect(document.activeElement).toBe(field);
  expect(mounted).toHaveBeenCalledTimes(1);
});

it("reconciles Main through existing read owners only when the zone changes", () => {
  const counts = vi.spyOn(useSectionsStore.getState(), "loadCounts").mockResolvedValue();
  const refresh = vi.spyOn(useItemsStore.getState(), "refresh").mockResolvedValue();
  const sourceCheck = vi.spyOn(useSectionsStore.getState(), "startSourceCheck");
  dispose.push(installDisplayZoneReconciliation());
  fireEvent.focus(window);
  expect(counts).not.toHaveBeenCalled();
  zone = "UTC";
  offset = 0;
  fireEvent.focus(window);
  expect(counts).toHaveBeenCalledTimes(1);
  expect(refresh).toHaveBeenCalledTimes(1);
  expect(sourceCheck).not.toHaveBeenCalled();
  vi.advanceTimersByTime(60_000);
  expect(counts).toHaveBeenCalledTimes(1);
});

it("shows times in a chosen display zone, and in the computer's under System or an unknown value", () => {
  const { dateTime } = createTranslator("en", "en-US");
  const instant = "2016-03-31T22:00:00.000Z";
  const changed = vi.fn();
  dispose.push(subscribeDisplayZone(changed));
  applyDisplayZoneSetting("Asia/Tokyo");
  expect(displayTimeZone()).toBe("Asia/Tokyo");
  expect(formatLocalMinute(instant, dateTime)).toContain("Apr 1, 2016");
  expect(changed).toHaveBeenCalledTimes(1);
  applyDisplayZoneSetting("UTC");
  expect(formatLocalMinute(instant, dateTime)).toContain("Mar 31, 2016");
  for (const setting of ["system", "Mars/Olympus", 9, undefined]) {
    applyDisplayZoneSetting(setting);
    expect(displayTimeZone()).toBeUndefined();
  }
  expect(changed).toHaveBeenCalledTimes(3);
  // Under a chosen zone the computer's zone still counts, for System entries.
  applyDisplayZoneSetting("UTC");
  zone = "Europe/Paris";
  offset = -120;
  fireEvent.focus(window);
  expect(changed).toHaveBeenCalledTimes(5);
});
