// Vitest global setup.
//
// The default `node` environment has no `navigator`; utilities that read the
// platform at module load must never throw under `node`, so a bare stub is
// installed here. Tests that need a specific platform stub it themselves.

import { vi } from "vitest";

if (typeof globalThis.navigator === "undefined") {
  vi.stubGlobal("navigator", { platform: "", userAgent: "" });
}
