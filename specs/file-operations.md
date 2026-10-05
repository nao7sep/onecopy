# File Operations

## Contract boundary

This contract begins when an owning review surface submits an accepted operation plan. The review surface owns selection, view-specific command meaning, and when confirmation is requested; the common confirmation safety below applies across review surfaces. A command with no modal freezes its plan when the command is accepted; when confirmation is required, accepting that confirmation freezes the plan. This contract owns the captured files and destinations and every filesystem effect that follows.

Confirmation freezes the complete batch from the physical copies, companion relationships, representative filenames, destinations, operation modes, and order OneCopy currently knows. Later discovery or background reconciliation does not broaden or reinterpret that batch. Missing preparation or incomplete optional information does not block an operation; OneCopy acts on the known files and relationships the user confirmed.

Visibility under `library-visibility.md` does not exempt known physical copies or locally paired companions from an accepted logical-item operation. Copy preserves them, Move handles covered sources, and deletion handles every planned copy as defined below; hidden non-identical items are not added to that scope.

File operations remain available while background work runs. Background work yields after its current bounded unit so the foreground operation can proceed. Until it does, the operation surface shows that it is waiting for background work, with Cancel; the operation never fails merely because background work is slow to yield, and cancelling while waiting performs no filesystem work. Only one file-changing operation is active at a time. A second mutation or library-index rebuild is refused with an explanation rather than queued against stale state.

## Current-file boundary

A planned source identifies a recorded path, not an immutable reviewed snapshot. OneCopy acts on the regular file currently present at that path and does not promise to prove that its bytes still match what was shown before confirmation. A missing, unreadable, locked, or unsuitable path fails independently.

A Move main output covers the item's other copies only when the delivered bytes match the logical item's recorded content. A planned copy whose bytes no longer match is neither delivered nor removed: it stays in place with an Issue, together with the companions paired with it, and the next planned copy is tried. When no planned copy still matches, the output is undelivered and every copy remains. Copy delivers the file as it currently exists, and an item whose content has not yet been identified delivers its current bytes.

Unavailable source directories or drives do not disable the application or invalidate every available copy. An operation uses the available planned sources, records unavailable paths, and leaves their handling to later reconciliation or a newly confirmed operation.

OneCopy records the physical volume identity of each configured source directory the first time it sees it. Every mutation admission, trash Empty, and source-check start re-verifies that identity before touching a file, scoped to the configured roots the work at hand actually touches: it refuses only work on a root where a different physical volume now answers at that configured path, where the recorded identity cannot be read at all, or where the drive does not answer the check in time — a failed check is never treated as verified-safe — and its notice names the affected folder or folders. Work that touches no unverifiable root proceeds normally, and a source check still checks every other configured root instead of stopping outright. A filesystem with no stable identity to read degrades to presence-only, as elsewhere in this contract.

## Drives that stop answering

A network share, a sleeping NAS, or a failing removable drive can stop answering without failing. Every filesystem step OneCopy takes on a configured source or destination has a time limit: about 15 seconds for a quick check (whether a file exists, its size and dates) and about 30 seconds for each read, write, rename, or removal, with the final flush of a large new file allowed longer in proportion to its size. A step that exceeds its limit is given up on and fails as a drive that is not responding; the drive answers again by itself once the step returns. While a drive has a step outstanding that was given up on, every later step on that drive fails at once as not responding instead of waiting again, and work on every other drive continues normally. Each configured source and destination folder counts as its own drive for this, however it is reached (a share mounted inside a home folder, an automount, a link to another volume), so a share that stops answering never holds up folders on the startup disk; two configured folders on one physical drive each wait out one step before failing at once.

Giving up on a step does not undo it: a rename or removal given up on may still complete later. OneCopy therefore reports such a file's **outcome as unknown** instead of guessing:

- A publication given up on leaves either the complete verified output at its final name or nothing there (a private output that never landed is removed once the step returns). The file counts as outcome unknown, its sources stay in place — a Move never handles a source after an unknown publication — and nothing further is written to that destination in the operation.
- A recoverable move or permanent deletion given up on leaves the file either in deleted-file storage (its provenance was recorded first) or still in place. The file counts as outcome unknown and its index entry stays; the next source check settles it as present or missing.

Each such file carries a current Issue saying its outcome is unknown. A step that was refused at once because its drive had not answered earlier has a known outcome: nothing happened, and it fails like any other unavailable file.

## Operation modes

Copy establishes the planned main and companion outputs and leaves every source in place.

Move establishes each output group before applying that group's requested source action. Ordinary Move cleanup sends its covered sources to recoverable deleted-file storage. An explicitly confirmed permanent variant deletes its covered sources without recoverable storage.

Ordinary deletion sends every planned main copy and every locally paired companion in the submitted logical item to recoverable deleted-file storage. Permanent deletion deletes those planned files without recoverable storage. Deletion may complete sequentially across physical files and can therefore produce an honest partial result.

New installations confirm direct single-item recoverable deletion by default; the user may disable that one confirmation. The preference applies only to a direct command targeting one explicitly selected item. Multi-item deletion, Comparison complement decisions, delete-every-visible, destination Move cleanup, overwrite displacement, and every permanent-deletion path always require an exact-scope review. Cancelling review performs no filesystem work and leaves the owning review state unchanged.

Destructive confirmations explicitly focus the safe footer Cancel action on entry, visibly, before the next key can activate anything. Left/Right moves among footer actions without activating them; Tab reaches the adjacent Delete action in one step. Enter activates only the focused action, and Escape cancels only the topmost confirmation. A held entry/deletion key never confirms a newly opened dialog. There is no bare D shortcut. These footer arrows are a deliberate OneCopy confirmation exception, not a general modal navigation rule.

## Main and companion outputs

The logical item's current representative supplies the main output filename. Differing names among byte-identical main copies do not block Copy or Move and do not create a filename vote.

Companion outputs are the union of the known companions paired locally with the planned main copies. Different companion output names may all be delivered. When several companions would use the same output name, the companion beside the highest-ranked main copy wins; if that copy lacks the name, preference continues through the established representative ordering. Companion contents are not compared merely to choose the output.

Copy leaves every companion source in place. A successfully established winning companion output covers every planned source companion represented by that output name; a failed output covers none of them. Move handles only those covered companion sources. Direct recoverable or permanent deletion handles every planned locally paired companion without comparing companion contents.

## Destination admission

At operation start, a destination must be a configured destination root or a selected descendant reached beneath one, must exist as a directory, and must be outside every configured source. The selected destination's current state is the authority for that operation; OneCopy does not maintain a durable destination-volume identity or continuously prove the physical identity of the directory and all of its ancestors.

Destination names follow the destination filesystem's natural case behavior. OneCopy does not impose cross-platform case equivalence on a filesystem that distinguishes case. On a case-insensitive destination, names that differ only by case are one name everywhere OneCopy plans: between selected items, against existing entries, among companions that would share an output name, and when choosing a Rename suffix. On macOS, names that differ only in Unicode normalization form (NFC vs. NFD) are likewise one name everywhere OneCopy plans, because APFS and HFS+ normalize names on the way to disk; this does not apply on Windows. An occupied exact destination path follows the reviewed conflict policy and is never replaced silently.

## Destination conflicts

Before filesystem work begins, OneCopy checks the complete selected set and presents every known destination conflict together. The user chooses one policy for the complete operation: Cancel, Rename and Copy/Move, or Overwrite. Overwrite is offered only when every conflict is with an existing regular file outside the selected set; when a conflict lies between selected items or the existing entry is not a regular file, the choices are Cancel and Rename. There is no Skip and no intentionally partial selected set. Cancel performs no filesystem work; resolving or cancelling expected conflicts does not itself create an Issue.

Rename treats the main output and its companion outputs as one family and applies one available suffix consistently. The default is `name 2.ext` on macOS and `name (2).ext` on Windows. One simple setting may choose between those styles; OneCopy does not expose an unrestricted filename format string.

Overwrite first prepares and read-back-verifies the complete replacement privately: every output of the item. It then sends the existing destination file and its companion family to recoverable deleted-file storage before publishing the verified replacement. When any output of the item cannot be prepared, nothing of that item is displaced; its conflicting outputs fail and the existing destination files stay in place. It never silently destroys the replaced destination group.

A freshly byte-verified existing output may count as already delivered. A newly confirmed Move retry may therefore finish only source cleanup after proving that the required destination bytes already exist; it does not duplicate the output, assume equality from names or metadata, or replay stale intent.

## Verified publication

Each main or distinct companion output is an independent output group. OneCopy writes one output completely under a private destination name, rereads it and proves that the new bytes match the selected source, then publishes it at the final name without overwriting another entry. An incomplete write must never appear as a finished destination file. Where the filesystem offers no exclusive rename (exFAT on macOS), OneCopy reserves the final name by creating it exclusively and replaces only that empty reservation, so an interruption at that instant can leave an empty file at the final name, never partial bytes. Private names are short and fixed in length: a final name the destination accepts can always be staged, and a final name it refuses fails as that one file.

Read-back verification is mandatory for every Copy and Move output. It is a correctness rule and has no user-disableable mode.

Every Copy and Move output keeps what a Finder copy keeps of the source it was written from: its modified time, its birth time where the platform has one, its permissions (on Windows, the read-only attribute), and on macOS its extended attributes, Finder tags among them. A destination that cannot hold some of these, such as an exFAT, FAT or network volume without native extended attributes, keeps what it can and always the modified time, and the rest is dropped without a warning or `._` files. An output whose modified time the destination refuses fails like any other write to that destination.

Before changing a file, OneCopy pauses and releases any app-owned media reader for that file. Source handling for Move begins only after the corresponding output group has been verified and published. A successful main output may therefore remain established and its covered main sources may be handled even if a later companion output fails; the failed companion sources remain in place.

## Failures and partial results

A failure tied to one planned file is recorded and skipped when later files have an independent chance to succeed. A failure that invalidates a shared requirement for the remaining batch, including an unusable destination, unavailable database, inability to save the promised failure record, or a new unreviewed destination conflict, stops the unstarted remainder. A destination write failure that indicates full, disconnected, or broken storage stops later writes to that destination.

Completed outputs, recoverable moves, permanent deletions, and source cleanups remain completed when later work fails or is cancelled. OneCopy does not copy completed outputs back, search deleted-file storage for rollback material, or represent the batch as atomic. Unattempted sources and sources whose required output failed remain in place.

One persistent nonmodal operation surface shows progress, Cancel, `Cancelling after current file…`, and the final completed, failed, outcome-unknown, and unstarted result. The result identifies completed work, preserved sources, failed files, files whose outcome is unknown (`Drives that stop answering`), and the unstarted remainder truthfully. A completed recoverable operation keeps its exact-count receipt visible and offers direct access to the stored files. A retry is a newly confirmed operation over current library and filesystem state, not a replay of stale destructive intent.

## Cancellation

Cancellation takes effect between physical files or other bounded filesystem steps. It does not interrupt a publication, recoverable move, or deletion halfway through its owned step: from a copy's final publication through a Move's cleanup of the sources it covers, Cancel lets each of those short steps finish within its own time limit. Writing and verifying a private output that is not yet published may stop mid-file, and Cancel also ends a read, write, or final flush of that private output that a drive is not answering or is still flushing, without waiting for the drive. OneCopy removes its unpublished private output on cancellation and on every failure, but never rolls back work that has already reached its completed boundary.

Because cancellation is bounded, a batch and even one logical item's physical copies may complete partially. The partial result follows the same accounting and recovery rules as any other failure.

## Recoverable storage and manual recovery

Recoverable deletion keeps each file under the most-specific configured root containing it. Source deletion and Move cleanup use the most-specific configured source root; overwrite displacement uses the selected configured destination root. The accepted operation plan freezes that root before filesystem work begins; the storage layer receives the frozen root and never guesses from the drive or application home.

The configured root is OneCopy's access boundary. Choosing a root authorizes discovery and file operations throughout its descendants, including descendants with narrower access than the configured root. OneCopy preserves the file's own access metadata as the filesystem permits, but does not infer separate user-access intent from nested directories, reproduce permissions inherited only from those directories, or make deleted files private to the current account. Configuring a broader root when users require exclusive access to its separate descendants is a configuration error rather than an access policy OneCopy can reconstruct.

Each configured root stores its deleted files beneath its own hidden `.onecopy-trash` directory. Before moving a file, OneCopy proves that the file is contained by the frozen root, that the move remains on the same physical filesystem however the root and file are spelled, and that the root's deleted-files directory is a real directory inside it rather than a link elsewhere; failed validation leaves the source untouched. The directory is created lazily beneath that root so its access remains constrained by the root's traversal and permission boundary. Source discovery, watchers, and destination browsing exclude these directories everywhere they occur, along with OneCopy's own data folder wherever a configured source happens to contain it (`library-maintenance.md`).

The application home does not own deleted-file storage. Two application homes configured for the same root intentionally see the same root-local location, while files protected by different configured roots never move into one shared drive-level or application-level directory.

OneCopy records the provenance of every stored file before moving it: its path relative to the root, its stored name, its size and modification time, why it was stored (deletion, Move cleanup, or Overwrite displacement), the operation and logical item it belonged to, whether it was a main copy or a companion, and the content hash OneCopy knew, including the reviewed hash of a file Overwrite displaced. Each record carries the version of its format. Records are only ever appended, never rewritten or pruned, and stay inside the day folder they describe, so removing a day folder by hand remains safe. Each move into deleted-file storage, each restore and each file Empty removes is also kept in OneCopy's own records. The latest record naming a stored file is the authoritative one; an earlier record for the same name describes nothing. OneCopy can reveal each known root-local location. Revealing creates an absent empty location before opening it, subject to the same configured-root validation. Files can be brought back with Restore (`Restore`) or manually with the operating system's file manager. OneCopy does not provide Undo.

OneCopy never automatically prunes or empties deleted-file storage. Emptying it is an explicit confirmed permanent action over the totals the confirmation showed: when the location gained or lost files after those totals were measured, nothing is removed and the new totals are shown for review. Cancellation takes effect between individual deletions. Users may also remove stored material outside OneCopy; doing so never triggers source deletion, operation replay, or automatic reconstruction.

## Restore

Restore brings chosen stored files back to exactly where they were, inside the same configured root. It never undoes or replays an operation, never restores to another folder or across roots, and never restores from a root that is not configured or not available.

Deleted files lists each available configured root's stored files separately, grouped by the local day of deletion and by deleted item: the main copies, their companions and the duplicate copies one operation removed together. Search matches the file name or the original folder. A stored file that changed since it was deleted, is no longer a regular file, whose original location lies inside deleted-file storage or OneCopy's own storage, or whose name cannot be reconstructed stays listed but cannot be restored; it offers Reveal only. A record whose stored file is gone is not listed, and stored files with no record are counted, not listed or restored. Records a newer OneCopy wrote are counted and left as they are, never read or rewritten.

Each selected file moves back to its original path by a rename on the same filesystem; bytes are never copied. Missing folders on the way are recreated inside the root. A file whose way is blocked by a link, a file, or a different drive at a folder on its original path is not restored. Restore never replaces anything:

- An original path already holding the same bytes is left alone; the stored file stays in Deleted files and is reported as already at its original location.
- An original path holding different content or anything that is not a regular file receives the restored file under the ordinary Rename suffix (`Destination conflicts`). A main file and the companions restored with it from one deleted item into one folder share one suffix.
- When several deleted versions of one path are restored together, the newest deletion takes the original name and the older ones take suffixes.

A review appears only when something needs a decision or a warning: name conflicts (Cancel or Rename and Restore), folders to be recreated, selected entries that will be skipped, or a companion that comes back under its original name although its main file was restored earlier under another, so the two no longer pair. Otherwise restoring starts directly: it destroys nothing, and a mistaken restore is undone by deleting again. Confirming freezes the plan the review showed; when a target or stored file changed since, nothing is restored and the new review is shown. A target that becomes occupied after confirmation fails that one file; it is never renamed silently.

Restore follows the rest of this contract: one file-changing operation at a time, Waiting for background work with Cancel, cancellation between files, the time limits of `Drives that stop answering` (a rename given up on leaves the file either in Deleted files or at its target, and its outcome is unknown), Issues for every file not restored, one Activity operation, and a result that counts restored, already there, failed, outcome-unknown and unstarted files truthfully with no rollback. A read-only or full drive or a root that went away stops the unstarted remainder in that root. Each restored file adds a record that it was restored.

A restored file in a source root re-enters the library through ordinary discovery; Restore re-reads the folders it restored into itself, so the library updates without waiting for the watcher. Its identity comes from ordinary hashing, never from the record, so content that already exists elsewhere simply becomes another copy of that item again, and companions pair again by the ordinary rules. A file restored into a destination root only reappears on disk.

## Normal exit and abnormal termination

Normal application exit stops admission of new mutation work, requests cancellation of the active operation, and shows `Finishing current file before exit…` while it waits for the current bounded filesystem step to finish. That wait is bounded to 30 seconds. A step that reaches its own safe point sooner lets exit proceed at once. A step that does not — a stalled volume mid-write, for example — is given up on at the deadline: OneCopy kills any subprocess it still owns and exits without finishing that one file. Its unpublished private output is never at the file's final name (`Verified publication`), so no partial file is ever visible there.

A leftover private staging or claim pathname is excluded from discovery everywhere, on every host, unconditionally: a live process of a different application home may still own it where two homes are configured to see the same root (`Recoverable storage and manual recovery`), so it is never indexed as library content regardless of who owns it. Removing it is a separate, ownership-proven decision: its name carries this application home's identity and the process id that created it, and only this application home's own file, naming a process no longer running, is ever removed — never a live process's file, and never a different application home's file. That sweep runs where the leftover actually lands: the next time this application home writes into the same destination folder, and the next time this application home's walk visits the same source root. App-owned media readers and other mutation resources are released before the process exits.

OneCopy does not persist or replay destructive operation plans after restart. Forced process termination, operating-system termination, power loss, exhausted process memory, and fatal operating-system or native-dependency failures are outside the normal-exit guarantee. Durable completed steps remain completed, and later startup reconciliation observes the resulting filesystem state.
