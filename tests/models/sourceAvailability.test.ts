import { describe, expect, it } from "vitest";
import { allCopiesUnavailable, pathUnavailable } from "../../src/models/sourceAvailability";

describe("source availability presentation", () => {
  it("marks an item only when every copy is under an unavailable source", () => {
    expect(allCopiesUnavailable(["/offline/photos", "/online/photos"], ["/offline"])).toBe(false);
    expect(allCopiesUnavailable(["/offline/a", "/offline/b"], ["/offline"])).toBe(true);
    expect(allCopiesUnavailable([], ["/offline"])).toBe(false);
    expect(pathUnavailable("/offline-other/photo.jpg", ["/offline"])).toBe(false);
    expect(pathUnavailable("/offline/photo.jpg", [])).toBe(false);
  });
  it("matches Windows components across path spellings", () => {
    expect(pathUnavailable("D:/Photos/shot.jpg", ["d:/photos/"])).toBe(true);
    expect(pathUnavailable("//?/UNC/SERVER/Photos/shot.jpg", ["//server/photos"])).toBe(true);
    expect(pathUnavailable("D:/Photos-other/shot.jpg", ["d:/photos"])).toBe(false);
  });
});
