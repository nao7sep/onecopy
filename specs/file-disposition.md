# File Disposition

- File operations remain available while background work runs. Foreground operations take priority after background work reaches a safe stopping point.
- Confirmation freezes the complete batch from the paths, copies, companions, filenames, and destinations OneCopy currently knows. Later discoveries do not join that batch.
- Missing information does not block an operation. OneCopy acts on the currently known files and relationships.
- OneCopy does not promise that a reviewed path still contains the same bytes when the operation begins. It acts on the regular file currently at that path; a missing, unreadable, or unsuitable path fails independently.
- Trash and permanent deletion process every currently known main copy and every companion paired locally with those copies.
- Trash recovery uses the operating system or OneCopy's external Trash location as applicable. OneCopy does not provide an in-app Restore command.
- Copy and Move choose the main filename from the logical item's current representative.
- Companion outputs are the union of the known local companions. When several companions would use the same output name, the companion beside the representative copy wins, followed by companions beside later-ranked copies. Companion contents are not compared merely to make this choice.
- Destination names follow the destination filesystem's natural case behavior. OneCopy never overwrites an existing exact destination path and does not impose universal case-insensitive uniqueness on filesystems that distinguish case.
- A destination must be configured outside the source folders when the operation starts. OneCopy does not continuously prove that the destination's physical identity or containment remains unchanged afterward.
- Each output is first written completely to a private temporary file at the destination, read back and verified, then published without overwriting another entry.
- Source cleanup for Move begins only after the corresponding output group has been verified. Successful output groups may be cleaned independently of failed groups.
- Completed output, Trash, deletion, and cleanup work is never rolled back merely because later work fails or is cancelled.
- Cancellation takes effect between physical files or other bounded filesystem steps. The result may therefore be partial and is reported honestly.
- Normal application exit stops admitting new mutation work and waits for the current bounded filesystem step to finish. No durable replay or rollback transaction is promised.
- A failure tied to one planned file is recorded and skipped when later files have an independent chance to succeed.
- A failure invalidating a shared requirement for the remaining batch—such as an unusable destination, unavailable database, or inability to save failure records—stops the unstarted remainder.
- An existing verification preference does not weaken mandatory output read-back. Any inert preference implying otherwise is removed.
