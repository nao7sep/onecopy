// The core serves the EFFECTIVE configuration: its stored values over the
// core's own defaults (storage::effective_config), in the startup load and in
// every save result. A reader here therefore never supplies a default of its
// own; an absent or wrong-shape member reads as off, empty, or null.

export type AppConfig = Record<string, unknown>;

export function configFlag(config: AppConfig | null | undefined, key: string): boolean {
  return config?.[key] === true;
}

export function configNumber(config: AppConfig | null | undefined, key: string): number | null {
  const value = config?.[key];
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

export function configString(config: AppConfig | null | undefined, key: string): string | null {
  const value = config?.[key];
  return typeof value === "string" ? value : null;
}

/** Whether a direct single-item recoverable Delete asks first
 * (on for new installations, and the user may turn it off). Every direct-Delete surface reads this one answer. */
export function confirmsTrashDelete(config: AppConfig | null | undefined): boolean {
  return configFlag(config, "confirmTrashDelete");
}
