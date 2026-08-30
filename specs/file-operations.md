# File Operations

## Contract boundary

This contract begins when an owning review surface submits a confirmed operation plan. The review surface owns selection, view-specific command meaning, and the presentation of confirmation. This contract owns the files and destinations captured by that confirmation and every filesystem effect that follows.

Confirmation freezes the complete batch from the physical copies, companion relationships, representative filenames, destinations, operation modes, and order OneCopy currently knows. Later discovery or background reconciliation does not broaden or reinterpret that batch. Missing preparation or incomplete optional information does not block an operation; OneCopy acts on the known files and relationships the user confirmed.

File operations remain available while background work runs. Background work yields after its current bounded unit so the foreground operation can proceed. Only one file-changing operation is active at a time.

## Current-file boundary

A planned source identifies a recorded path, not an immutable reviewed snapshot. OneCopy acts on the regular file currently present at that path and does not promise to prove that its bytes still match what was shown before confirmation. A missing, unreadable, locked, or unsuitable path fails independently.

Unavailable source directories or drives do not disable the application or invalidate every available copy. An operation uses the available planned sources, records unavailable paths, and leaves their handling to later reconciliation or a newly confirmed operation.

## Operation modes

Copy establishes the planned main and companion outputs and leaves every source in place.

Move establishes each output group before applying that group's requested source action. Ordinary Move cleanup sends its covered sources to recoverable Trash. An explicitly confirmed permanent variant deletes its covered sources without Trash.

Ordinary deletion sends every planned main copy and every locally paired companion in the submitted logical item to recoverable Trash. Permanent deletion deletes those planned files without Trash. Deletion may complete sequentially across physical files and can therefore produce an honest partial result.

Ordinary Trash follows the configured Trash-confirmation policy. Every permanent-deletion path requires explicit confirmation. Cancelling confirmation performs no filesystem work and leaves the owning review state unchanged.

## Main and companion outputs

The logical item's current representative supplies the main output filename. Differing names among byte-identical main copies do not block Copy or Move and do not create a filename vote.

Companion outputs are the union of the known companions paired locally with the planned main copies. Different companion output names may all be delivered. When several companions would use the same output name, the companion beside the highest-ranked main copy wins; if that copy lacks the name, preference continues through the established representative ordering. Companion contents are not compared merely to choose the output.

Copy leaves every companion source in place. Move handles only the companion sources covered by a successfully established companion output. Direct Trash or permanent deletion handles every planned locally paired companion without comparing companion contents.

## Destination admission

At operation start, a destination must be configured, must exist as a directory, and must be outside every configured source. The selected destination's current state is the authority for that operation; OneCopy does not maintain a durable destination-volume identity or continuously prove the physical identity of the directory and all of its ancestors.

Destination names follow the destination filesystem's natural case behavior. OneCopy does not impose cross-platform case equivalence on a filesystem that distinguishes case. Publication never replaces an occupied exact destination path.

## Verified publication

Each main or distinct companion output is an independent output group. OneCopy writes one output completely under a private destination name, rereads it and proves that the new bytes match the selected source, then publishes it at the final name without overwriting another entry. An incomplete write must never appear as a finished destination file.

Read-back verification is mandatory for every Copy and Move output. It is a correctness rule and has no user-disableable mode.

Before changing a file, OneCopy pauses and releases any app-owned media reader for that file. Source handling for Move begins only after the corresponding output group has been verified and published. A successful main output may therefore remain established and its covered main sources may be handled even if a later companion output fails; the failed companion sources remain in place.

## Failures and partial results

A failure tied to one planned file is recorded and skipped when later files have an independent chance to succeed. A failure that invalidates a shared requirement for the remaining batch, including an unusable destination, unavailable database, or inability to save the promised failure record, stops the unstarted remainder. A destination write failure that indicates full, disconnected, or broken storage stops later writes to that destination.

Completed outputs, Trash moves, permanent deletions, and source cleanups remain completed when later work fails or is cancelled. OneCopy does not copy completed outputs back, search Trash for rollback material, or represent the batch as atomic. Unattempted sources and sources whose required output failed remain in place.

The result identifies completed work, preserved sources, failed files, and the unstarted remainder truthfully. A retry is a newly confirmed operation over current library and filesystem state, not a replay of stale destructive intent.

## Cancellation

Cancellation takes effect between physical files or other bounded filesystem steps. It does not terminate a write, publication, Trash move, or deletion halfway through its owned step. OneCopy removes or abandons its unpublished private output as appropriate, but never rolls back work that has already reached its completed boundary.

Because cancellation is bounded, a batch and even one logical item's physical copies may complete partially. The partial result follows the same accounting and recovery rules as any other failure.

## Trash and manual recovery

Ordinary Trash is recoverable until the relevant stored file or its recovery evidence is removed. OneCopy records enough provenance to associate stored files with their original locations and can reveal the relevant Trash location. Recovery is performed manually with the operating system's file manager; OneCopy does not provide Restore or Undo.

OneCopy never automatically prunes or empties Trash it manages. Emptying is an explicit permanent action, and cancellation takes effect between individual deletions. Users may also remove Trash material outside OneCopy; doing so never triggers source deletion, operation replay, or automatic reconstruction.

## Normal exit and abnormal termination

Normal application exit stops admission of new mutation work, requests cancellation of the active operation, waits for the current bounded filesystem step to finish, and waits until OneCopy no longer owns an unsafe in-progress mutation. App-owned media readers and other mutation resources are released before the process exits.

OneCopy does not persist or replay destructive operation plans after restart. Forced process termination, operating-system termination, power loss, exhausted process memory, and fatal operating-system or native-dependency failures are outside the normal-exit guarantee. Durable completed steps remain completed, and later startup reconciliation observes the resulting filesystem state.
