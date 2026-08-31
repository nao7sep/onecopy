# File Operations

## Contract boundary

This contract begins when an owning review surface submits an accepted operation plan. The review surface owns selection, view-specific command meaning, and the presentation of confirmation. A command with no modal freezes its plan when the command is accepted; when confirmation is required, accepting that confirmation freezes the plan. This contract owns the captured files and destinations and every filesystem effect that follows.

Confirmation freezes the complete batch from the physical copies, companion relationships, representative filenames, destinations, operation modes, and order OneCopy currently knows. Later discovery or background reconciliation does not broaden or reinterpret that batch. Missing preparation or incomplete optional information does not block an operation; OneCopy acts on the known files and relationships the user confirmed.

File operations remain available while background work runs. Background work yields after its current bounded unit so the foreground operation can proceed. Only one file-changing operation is active at a time. A second mutation or library-index rebuild is refused with an explanation rather than queued against stale state.

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

Copy leaves every companion source in place. A successfully established winning companion output covers every planned source companion represented by that output name; a failed output covers none of them. Move handles only those covered companion sources. Direct Trash or permanent deletion handles every planned locally paired companion without comparing companion contents.

## Destination admission

At operation start, a destination must be a configured destination root or a selected descendant reached beneath one, must exist as a directory, and must be outside every configured source. The selected destination's current state is the authority for that operation; OneCopy does not maintain a durable destination-volume identity or continuously prove the physical identity of the directory and all of its ancestors.

Destination names follow the destination filesystem's natural case behavior. OneCopy does not impose cross-platform case equivalence on a filesystem that distinguishes case. An occupied exact destination path follows the reviewed conflict policy and is never replaced silently.

## Destination conflicts

Before filesystem work begins, OneCopy checks the complete selected set and presents every known destination conflict together. The user chooses one policy for the complete operation: Cancel, Rename and Copy/Move, or Overwrite. There is no Skip and no intentionally partial selected set. Cancel performs no filesystem work; resolving or cancelling expected conflicts does not itself create an Issue.

Rename treats the main output and its companion outputs as one family and applies one available suffix consistently. The default is `name 2.ext` on macOS and `name (2).ext` on Windows. One simple setting may choose between those styles; OneCopy does not expose an unrestricted filename format string.

Overwrite first prepares and read-back-verifies the complete replacement privately. It then sends the existing destination file and its companion family to recoverable Trash before publishing the verified replacement. It never silently destroys the replaced destination group.

A freshly byte-verified existing output may count as already delivered. A newly confirmed Move retry may therefore finish only source cleanup after proving that the required destination bytes already exist; it does not duplicate the output, assume equality from names or metadata, or replay stale intent.

## Verified publication

Each main or distinct companion output is an independent output group. OneCopy writes one output completely under a private destination name, rereads it and proves that the new bytes match the selected source, then publishes it at the final name without overwriting another entry. An incomplete write must never appear as a finished destination file.

Read-back verification is mandatory for every Copy and Move output. It is a correctness rule and has no user-disableable mode.

Before changing a file, OneCopy pauses and releases any app-owned media reader for that file. Source handling for Move begins only after the corresponding output group has been verified and published. A successful main output may therefore remain established and its covered main sources may be handled even if a later companion output fails; the failed companion sources remain in place.

## Failures and partial results

A failure tied to one planned file is recorded and skipped when later files have an independent chance to succeed. A failure that invalidates a shared requirement for the remaining batch, including an unusable destination, unavailable database, inability to save the promised failure record, or a new unreviewed destination conflict, stops the unstarted remainder. A destination write failure that indicates full, disconnected, or broken storage stops later writes to that destination.

Completed outputs, Trash moves, permanent deletions, and source cleanups remain completed when later work fails or is cancelled. OneCopy does not copy completed outputs back, search Trash for rollback material, or represent the batch as atomic. Unattempted sources and sources whose required output failed remain in place.

One persistent nonmodal operation surface shows progress, Cancel, `Cancelling after current file…`, and the final completed, failed, and unstarted result. The result identifies completed work, preserved sources, failed files, and the unstarted remainder truthfully. A retry is a newly confirmed operation over current library and filesystem state, not a replay of stale destructive intent.

## Cancellation

Cancellation takes effect between physical files or other bounded filesystem steps. It does not terminate a write, publication, Trash move, or deletion halfway through its owned step. OneCopy removes or abandons its unpublished private output as appropriate, but never rolls back work that has already reached its completed boundary.

Because cancellation is bounded, a batch and even one logical item's physical copies may complete partially. The partial result follows the same accounting and recovery rules as any other failure.

## Trash and manual recovery

Ordinary Trash uses OneCopy-managed storage on the same volume as each source and remains recoverable until the relevant stored file or its recovery evidence is removed. OneCopy records enough provenance to associate stored files with their original locations and can reveal the relevant per-volume Trash location. Recovery is performed manually with the operating system's file manager; OneCopy does not provide Restore or Undo.

OneCopy never automatically prunes or empties Trash it manages. `Empty OneCopy Trash…` is an explicit confirmed permanent action in Settings or maintenance, and cancellation takes effect between individual deletions. Users may also remove Trash material outside OneCopy; doing so never triggers source deletion, operation replay, or automatic reconstruction.

## Normal exit and abnormal termination

Normal application exit stops admission of new mutation work, requests cancellation of the active operation, shows `Finishing current file before exit…`, waits for the current bounded filesystem step to finish, and waits until OneCopy no longer owns an unsafe in-progress mutation. The user may return to the app when the platform permits, but OneCopy provides no force-exit control that weakens this boundary. App-owned media readers and other mutation resources are released before the process exits.

OneCopy does not persist or replay destructive operation plans after restart. Forced process termination, operating-system termination, power loss, exhausted process memory, and fatal operating-system or native-dependency failures are outside the normal-exit guarantee. Durable completed steps remain completed, and later startup reconciliation observes the resulting filesystem state.
