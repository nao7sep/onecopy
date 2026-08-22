import { describe, expect, it } from "vitest";
import { stringArrayField } from "../../src/utils/configProjection";

describe("durable config string-array projection", () => {
  it("keeps literal strings, filters wrong-shape members, and never mutates the document", () => {
    const config: Record<string, unknown> = {
      sourceDirs: [" /literal/space ", 123, null, { path: "/wrong" }, "/valid"],
      unknownFutureKey: { preserved: true },
    };

    expect(stringArrayField(config, "sourceDirs")).toEqual([
      " /literal/space ",
      "/valid",
    ]);
    expect(config).toEqual({
      sourceDirs: [" /literal/space ", 123, null, { path: "/wrong" }, "/valid"],
      unknownFutureKey: { preserved: true },
    });
    expect(stringArrayField(config, "destinationRoots")).toEqual([]);
  });
});
