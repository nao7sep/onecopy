# Library Reconciliation

- Files with identical content bytes form one logical item, regardless of path, filename case, or timestamps. Until content has been read, OneCopy may know only the individual physical file.
- Each physical copy keeps its own date evidence. OneCopy chooses the oldest acceptable resolved date among the available copies.
- The copy supplying that date also supplies the logical item's displayed and output filename.
- Equal dates are resolved by sorting complete paths case-insensitively, followed by exact path sorting. There is no random or drive-preference rule.
- If the chosen copy disappears and that disappearance is detected, the next copy under the same ordering becomes representative.
- A logical item with unchecked date evidence is pending. A logical item whose available copies have been checked without finding an acceptable date is completed Undated work. These states remain distinct so completed Undated items are not repeatedly read.
- Changing date-selection settings recomputes dates from saved evidence without rereading files. New or apparently changed files may provide new evidence when processed.
- Ordinary companions require the same directory and a case-insensitively identical filename stem. Pairing never crosses directories.
- Live Photos are the exception: files in one directory may be paired by an exact matching embedded Apple content identifier even when their names differ.
- Companion files paired beside different byte-identical copies do not need identical contents.
- "Check source folders" finds new files, missing files, and files whose recorded size or modification time changed. Unchanged files are not reread.
- Checking source folders runs outside the main-window startup path, may start automatically according to Settings, and has Start and Stop controls. Stopping preserves everything already discovered.
- "Complete file information" independently completes missing content identities, metadata, dates, and companion relationships. It has Pause and Resume controls.
- Stopping the source-folder check does not prevent already-discovered or watcher-discovered work from being completed.
- Watcher-driven updates remain active while OneCopy is open. A watcher failure becomes visible rather than silently leaving the library stale.
- A left-menu item is a section. "Recheck this section," also available through Cmd/Ctrl+R, rechecks the filesystem locations represented by that section. While all source folders are already being checked, the command is unavailable with an explanatory label instead of waiting invisibly.
- "Rebuild library index…" belongs in Settings. It rebuilds reconstructible library information and clears Issues without changing user files, Settings, managed tools, or user-authored choices. It cannot overlap an active file operation.
