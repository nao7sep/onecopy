export { log, toErrorFields, initLogging, reportWindowCall } from "./logging";
export { focusWhileActive } from "./window-focus";
export type { LogFields } from "./logging";
export { loadAppData, saveConfigFile, patchStateFile } from "./app-data";
export type {
  AiAccelerationCapability,
  BootstrapData,
  LoadedAppData,
  QuarantineRecord,
  StartupFailure,
} from "./app-data";
export {
  finishActivityOperation,
  latestActivityOperationId,
  newActivityOperationId,
  recordActivity,
} from "./activity";
export type {
  ActivityDraft,
} from "./activity";
