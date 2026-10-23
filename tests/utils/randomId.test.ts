import { afterEach, expect, it, vi } from "vitest";
import { randomId } from "../../src/utils/randomId";
import { newActivityOperationId, latestActivityOperationId } from "../../src/repositories/activity";

afterEach(() => vi.unstubAllGlobals());

it("uses the native UUID method when available", () => {
  const randomUUID = vi.fn(() => "native-uuid");
  vi.stubGlobal("crypto", { randomUUID });
  expect(randomId()).toBe("native-uuid");
  expect(randomUUID).toHaveBeenCalledOnce();
});

it("creates version-four operation IDs from secure bytes without randomUUID", () => {
  const getRandomValues = vi.fn((bytes: Uint8Array) => {
    bytes.set(Array.from({ length: 16 }, (_, index) => index * 17));
    return bytes;
  });
  vi.stubGlobal("crypto", { getRandomValues });
  const id = newActivityOperationId("settings");
  expect(id).toBe("settings:00112233-4455-4677-8899-aabbccddeeff");
  expect(latestActivityOperationId("settings")).toBe(id);
  expect(getRandomValues).toHaveBeenCalledOnce();
  expect(getRandomValues.mock.calls[0]![0]).toHaveLength(16);
});
