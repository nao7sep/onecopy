import { expect, it } from "vitest";
import { passiveScrollKey } from "../../src/utils/passiveScrollKey";

it("maps transcript scrolling keys without claiming unrelated keys", () => {
  expect(passiveScrollKey("ArrowDown", false, 200)).toBe(40);
  expect(passiveScrollKey("PageUp", false, 200)).toBe(-180);
  expect(passiveScrollKey(" ", true, 200)).toBe(-180);
  expect(passiveScrollKey("Home", false, 200)).toBe("start");
  expect(passiveScrollKey("Enter", false, 200)).toBeNull();
});
