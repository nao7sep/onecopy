// The frontend reads the core's EFFECTIVE configuration (src/models/config.ts).
// Tests build theirs from the core's defaults, pinned in this fixture and
// checked against DefaultConfig by the Rust suite
// (storage_tests::the_frontend_config_fixture_matches_the_core_defaults).
import defaults from "../fixtures/effective-config.json";
import { useAppStore } from "../../src/state/app-store";

export const DEFAULT_CONFIG: Readonly<Record<string, unknown>> = defaults;

export function effectiveConfig(
  overrides: Record<string, unknown> = {},
): Record<string, unknown> {
  return { ...structuredClone(defaults), ...overrides };
}

/** Seeds Main's loaded data with an effective configuration. */
export function seedAppConfig(overrides: Record<string, unknown> = {}): void {
  useAppStore.setState({
    appData: {
      config: effectiveConfig(overrides),
      state: {},
      dataRoot: "/app",
      debugEnabled: false,
      quarantines: [],
    },
  });
}
