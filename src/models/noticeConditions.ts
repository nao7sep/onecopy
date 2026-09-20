import type { MessageKey } from "../i18n/catalogues";

// The conditions the core reports. It records its own English with each
// notice and Issue, for history and for merging repeats; the interface says
// the condition in the reader's language, and shows what the core recorded
// only when that adds something its own sentence does not.
const CONDITION_KEYS: Record<string, MessageKey> = {
  "background-work-state-failed": "notice.fileInformationStateFailed",
  "config-save-failed": "notice.configSaveFailed",
  "dependency-install-failed": "notice.dependencyInstallFailed",
  "derived-worker-failed": "notice.derivedWorkerFailed",
  "event-delivery-failed": "notice.eventDeliveryFailed",
  "external-open-failed": "notice.externalOpenFailed",
  "file-information-state-failed": "notice.fileInformationStateFailed",
  "file-operation-state-failed": "notice.fileOperationStateFailed",
  "instance-activation-failed": "notice.instanceActivationFailed",
  "instance-listener-failed": "notice.instanceActivationFailed",
  "interface-failed": "notice.interfaceFailed",
  "media-use-state-failed": "notice.mediaUseStateFailed",
  "shutdown-media-release-failed": "notice.shutdownMediaReleaseFailed",
  "shutdown-window-recovery-failed": "notice.shutdownMediaReleaseFailed",
  "shutdown-worker-failed": "notice.shutdownMediaReleaseFailed",
  "sleep-prevention-failed": "notice.sleepPreventionFailed",
  "source-check-failed": "notice.sourceCheckFailed",
  "source-check-feedback-failed": "notice.sourceCheckFeedbackFailed",
  "state-save-failed": "notice.configSaveFailed",
  "text-preview-failed": "notice.textPreviewFailed",
  "transcription-worker-failed": "notice.transcriptionWorkerFailed",
  "trash-empty-entry-failed": "notice.fileOperationStateFailed",
  "update-check-failed": "notice.dependencyInstallFailed",
  "watcher-failed": "notice.sourceCheckFailed",
  "watcher-root-failed": "notice.sourceCheckFailed",
};

// Anything the core reports without naming a condition it has a sentence for.
const UNNAMED_CONDITION: MessageKey = "notice.backgroundStopped";

export function conditionKey(kind: string): MessageKey | null {
  return CONDITION_KEYS[kind] ?? null;
}

// The words for a notice or an Issue: the condition's own sentence when the
// core named one, otherwise what it recorded.
export function conditionText(
  kind: string,
  recorded: string | null,
  t: (key: MessageKey) => string,
): string {
  const key = conditionKey(kind);
  if (key !== null) return t(key);
  return recorded ?? t(UNNAMED_CONDITION);
}
