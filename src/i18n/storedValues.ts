import { documentTranslator } from "./I18nContext";
import type { MessageValues } from "./translate";

/** JSON-safe interpolation values: what actually crosses IPC and gets stored
 * beside a message key. A nested Message value (rare — only OneCopy's own
 * local last-resort path uses one) is flattened to text at record time, since
 * there is nowhere durable to keep ITS OWN key once serialized; everything
 * else round-trips exactly. */
export type StoredMessageValues = Record<string, string | number>;

export function storedValues(values: MessageValues | undefined): StoredMessageValues | undefined {
  if (values === undefined) return undefined;
  const flattened: StoredMessageValues = {};
  for (const [name, value] of Object.entries(values)) {
    flattened[name] = typeof value === "object" ? documentTranslator().text(value) : value;
  }
  return flattened;
}
