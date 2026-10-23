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

function isIdentityKey(value: unknown): value is string {
  if (typeof value !== "string") return false;
  try {
    identityFromKey(value);
    return true;
  } catch {
    // Persisted recovery hints are optional; an invalid neighbor must not
    // prevent loading the saved section and its independently valid anchor.
    return false;
  }
}

export function parseAnchorContext(value: unknown): AnchorContext | null {
  if (typeof value !== "object" || value === null) return null;
  const record = value as Record<string, unknown>;
  if (!Number.isInteger(record.index) || (record.index as number) < 0) return null;
  if (!Array.isArray(record.before) || !record.before.every(isIdentityKey)) {
    return null;
  }
  if (!Array.isArray(record.after) || !record.after.every(isIdentityKey)) {
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
