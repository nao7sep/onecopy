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


it("preserves Unix backslashes and case when deciding containment", () => {
  expect(pathUnavailable("/photos\\trip/shot.jpg", ["/photos/trip"])).toBe(false);
  expect(pathUnavailable("/photos/trip/shot.jpg", ["/photos\\trip"])).toBe(false);
  expect(pathUnavailable("/photos\\trip/shot.jpg", ["/photos\\trip/"])).toBe(true);
  expect(pathUnavailable("/Photos/trip/shot.jpg", ["/photos"])).toBe(false);
});

it("keeps encoded invalid native names distinct from each other and replacement characters", () => {
  const roots = ["/photos/\u0000bfe", "/photos/\u0000bff", "/photos/\ufffd"];
  for (const root of roots) {
    for (const other of roots) {
      expect(pathUnavailable(`${root}/shot.jpg`, [other])).toBe(root === other);
    }
  }
  expect(pathUnavailable("C:\\Photos\\\u0000wd800\\shot.jpg", ["c:/photos/\u0000wd800"])).toBe(true);
  expect(pathUnavailable("C:\\Photos\\\u0000wd801\\shot.jpg", ["c:/photos/\u0000wd800"])).toBe(false);
});

it("recognizes native Windows drive, UNC and verbatim prefixes before separator normalization", () => {
  expect(pathUnavailable("D:\\Photos\\shot.jpg", ["d:/photos"])).toBe(true);
  expect(pathUnavailable("\\\\SERVER\\Photos\\shot.jpg", ["//server/photos"])).toBe(true);
  expect(pathUnavailable("\\\\?\\D:\\Photos\\shot.jpg", ["d:/photos"])).toBe(true);
  expect(pathUnavailable("\\\\?\\UNC\\SERVER\\Photos\\shot.jpg", ["//server/photos"])).toBe(true);
});
