# Library Reconciliation

## Logical identity, dates, and companions

- Files with identical content bytes form one logical item, regardless of path, filename case, or timestamps. Until content has been read, OneCopy may know only the individual physical file.
- Each physical copy keeps its own date evidence. OneCopy chooses the oldest acceptable resolved date among the available copies.
- The copy supplying that date also supplies the logical item's displayed and output filename.
- Equal dates are resolved by sorting complete paths case-insensitively, followed by exact path sorting. There is no random or drive-preference rule.
- If no copy has an acceptable date, the same case-insensitive then exact complete-path order selects the representative and displayed/output filename while the logical item remains Undated.
- If the chosen copy disappears and that disappearance is detected, the next copy under the same ordering becomes representative.
- A logical item with unchecked date evidence is pending. A logical item whose available copies have been checked without finding an acceptable date is completed Undated work. These states remain distinct so completed Undated items are not repeatedly read.
- Date acceptability follows the configured date-selection policy, including its user-configurable earliest acceptable date. Changing that policy recomputes dates from saved evidence without rereading files. New or apparently changed files may provide new evidence when processed.
- Ordinary companions require the same directory and a case-insensitively identical filename stem. Pairing never crosses directories.
- Live Photos are the exception: files in one directory may be paired by an exact matching embedded Apple content identifier even when their names differ.
- Companion files paired beside different byte-identical copies do not need identical contents.

## Discovery and completion

- "Check source folders" finds new files, missing files, and files whose recorded size or modification time changed. Unchanged files are not reread.
- Checking source folders runs outside the main-window startup path, may start automatically according to Settings, and has Start and Stop controls. Stopping preserves everything already discovered.
- An unavailable or physically changed configured source root does not block the application. OneCopy works with reachable copies, records unavailable roots or paths, and reincorporates copies when they are detected again; it does not require a remembered physical-drive identity to return at that path.
- "Complete file information" independently completes missing content identities, metadata, dates, and companion relationships. It has Pause and Resume controls.
- Stopping the source-folder check does not prevent already-discovered or watcher-discovered work from being completed.
- Watcher-driven updates remain active while OneCopy is open. A watcher failure becomes visible rather than silently leaving the library stale.
- A left-menu item is a section. "Recheck this section," also available through Cmd/Ctrl+R, rechecks the filesystem locations represented by that section. While all source folders are already being checked, the command is unavailable with an explanatory label instead of waiting invisibly.
- Audio remains in Other files even when it receives a playable or transcript presentation. Preview detection or fallback never changes an item's library section.
- "Rebuild library index…" belongs in Settings. It rebuilds reconstructible library information and clears the reconstructible failure records governed by `failure-reporting-and-recovery.md` without changing user files, Settings, managed tools, or user-authored choices. It cannot overlap a file operation and quiesces index-owning background work before replacing the index; watcher changes observed during that boundary are processed afterward.

## Required preparation and optional enrichment

- Required preparation is part of a usable and truthful library. OneCopy does not offer durable switches that disable source reconciliation, file-information completion, watcher updates, image thumbnails, screen-sized image previews, video posters and playable preparation, or the appropriate playable, bounded-text, specialized, or attributes presentation for a displayed Other file.
- Suitable required background work may be paused or stopped temporarily. The interface states plainly which information or presentation remains incomplete until it resumes.
- Optional enrichment consists of video scene snapshots, similar-photo analysis, advisory face scoring, automatic video transcription, and automatic audio transcription. Each has an independent durable setting and defaults on when runnable.
- Automatic video and audio transcription are separate settings even though they share the managed transcription service and content-addressed cache. Exact copies share one completed transcript.
- The first-launch wizard explains required preparation without switches. A separate Additional features area controls optional enrichment, mirrored in Settings.
- OneCopy starts an optional feature off only for a concrete detected obstacle such as a missing tool or model, unsupported runtime, inadequate free storage, or memory below an enforced safety requirement, and explains that reason. Temporary battery or load conditions defer work without changing its durable setting.
- A warned feature may be deliberately enabled, but the app retains bounded concurrency, containment, and resource-safety limits. Managed tools and models are installed only through an explicit user action.

## Scheduling and controls

- Foreground file mutations outrank all derived work. Missing required presentation for the selected or visible item outranks optional enrichment; selected, visible, and nearby active-section items outrank distant library work.
- For images, required work orders file facts before thumbnail before screen preview. For videos, it orders facts before poster before playable preview. For audio and Other files, it orders facts before the applicable playable, specialized, bounded-text, or attributes presentation.
- Visible optional image work completes similarity/group facts before advisory face scoring. Visible optional video work completes scene snapshots before transcription. Audio transcription is optional enrichment.
- Active-section work proceeds outward through a bounded neighborhood around the anchor, then through the rest of the section. Remaining required and enabled optional queues receive bounded fair turns across work classes and sections; an earlier section or work class never monopolizes the worker until its entire backlog is empty.
- A new section, sort order, selection, or viewport replaces pending priority hints after the current bounded step. Distant work yields resources during active use, while nearby work may continue and idle time may use available background capacity.
- Only one CPU-heavy derived operation runs at a time unless measured evidence later establishes a wider safe limit.
- Required visible preparation preempts optional work at its next safe boundary. An interrupted automatic job publishes no partial result and returns to its queue; a user-requested optional job yields through the same coordinator rather than creating a second engine. Navigation among already prepared items does not churn the queue.
- Background Work separately exposes Check source folders with Start/Stop; Complete file information with Pause/Resume; required thumbnails, previews, and posters with Pause/Resume; and each enabled optional enrichment with Pause/Resume.
- Settings owns whether optional enrichment is enabled. Background Work owns temporary running state and reports the current item, nearby priority, completed and remaining counts, prerequisite blocking, and waits behind another heavy operation.
- The status-bar progress indicator opens Background Work.

## Transcription work

- When automatic transcription is enabled for the medium, selecting a pending file raises its existing job's priority instead of showing a redundant Transcribe action.
- When automatic transcription is disabled for the medium, Transcribe this file queues the same shared worker. A manual request waits with priority behind a running transcription rather than starting another engine.
- Paused transcription disables the item action with an explanation and Resume path. A failed attempt offers Retry and remains visible through Issues.
- Re-transcribe is a secondary item action. An existing completed transcript remains available while its replacement is built and is replaced only after complete success.
- Successful no-speech detection is completed work, not an indefinitely pending or repeatedly retried item.
