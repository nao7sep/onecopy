import type { DependencyState } from "../../src/state/binaries-store";

/** Synthetic versions: presentation evidence only, never upstream facts. */
export function managedToolsFixture(
  platform: "macos" | "windows" = "macos",
  identity: "known" | "unreadable" | "long" = "known",
): DependencyState[] {
  const pinned = (
    id: string, label: string, released: string, downloadBytes: number,
    kind: "model" | "runtime" = "model",
  ): DependencyState => ({
    id, label, kind, released, downloadBytes,
    status: "not-installed", installedVersion: null,
    facts: { latestKnownVersion: null, lastCheckedAtUtc: null },
    path: "", requiredForCore: false, checkable: false,
  });
  const ffmpeg: DependencyState = {
    id: "ffmpeg", label: "ffmpeg", kind: "binary",
    status: identity === "unreadable" ? "installed-unchecked" : "update-available",
    installedVersion: identity === "unreadable" ? null : platform === "windows"
      ? "Latest Auto-Build (2026-09-01 12:00)" : "9.0",
    facts: {
      latestKnownVersion: identity === "long"
        ? "synthetic-build-with-an-intentionally-long-identity-for-layout-verification-1234567890"
        : platform === "windows" ? "Latest Auto-Build (2026-09-08 12:00)" : "9.1",
      lastCheckedAtUtc: "2026-09-09T00:00:00.000Z",
    },
    path: "", requiredForCore: true, checkable: true, released: null, downloadBytes: null,
  };
  return [
    ffmpeg,
    ...(platform === "windows" ? [pinned(
      "onnxruntime-win-x64", "Face-scoring runtime (ONNX Runtime 1.28)",
      "2026-07-25", 139_145_017, "runtime",
    )] : []),
    pinned("whisper-large-v3-turbo", "Transcription model (Whisper large-v3-turbo)", "2024-10-01", 1_624_555_275),
    pinned("ultraface-rfb640", "Face detector (UltraFace RFB-640)", "2020-12-17", 1_588_012),
    pinned("hsemotion-enet-b2", "Expression model (HSEmotion EfficientNet-B2)", "2022-11-09", 30_779_724),
  ];
}
