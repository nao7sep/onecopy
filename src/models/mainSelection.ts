export interface AnchorContext {
  index: number;
  before: string[];
  after: string[];
}

export interface SectionMemory {
  anchor: string | null;
  context: AnchorContext | null;
}

const NEIGHBOR_LIMIT = 64;

export function parseAnchorContext(value: unknown): AnchorContext | null {
  if (typeof value !== "object" || value === null) return null;
  const record = value as Record<string, unknown>;
  if (!Number.isInteger(record.index) || (record.index as number) < 0) return null;
  if (!Array.isArray(record.before) || !record.before.every((key) => typeof key === "string")) {
    return null;
  }
  if (!Array.isArray(record.after) || !record.after.every((key) => typeof key === "string")) {
    return null;
  }
  return {
    index: record.index as number,
    before: record.before.slice(0, NEIGHBOR_LIMIT) as string[],
    after: record.after.slice(0, NEIGHBOR_LIMIT) as string[],
  };
}

export function anchorContextPayload(
  context: AnchorContext | null,
): SectionRecoveryContextPayload | null {
  if (context === null) return null;
  return {
    index: context.index,
    before: context.before.map(identityFromKey),
    after: context.after.map(identityFromKey),
  };
}

export function anchorContextFromPayload(
  context: SectionRecoveryContextPayload | null,
): AnchorContext | null {
  if (context === null) return null;
  return {
    index: context.index,
    before: context.before.map(identityKey),
    after: context.after.map(identityKey),
  };
}
import {
  identityFromKey,
  identityKey,
  type SectionRecoveryContextPayload,
} from "./items";
