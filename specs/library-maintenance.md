# Library Maintenance

This contract defines how OneCopy discovers source changes, completes known file information, prepares required presentation data, schedules optional enrichment, generates transcripts, exposes Background Work controls, and rebuilds reconstructible library state. Logical-item calculation is owned by `library-items.md`; completed-content presentation is owned by `content-presentation.md`; general failure presentation is owned by `failures-and-recovery.md`.

Background work & tools is the single surface for job controls and managed dependencies. Missing dependencies offer explicit installation beside the blocked work; opening or closing the surface never starts or cancels a download. Detailed queue totals are read only while the surface is open; the status bar follows runtime events.

A model-load failure pauses only the affected class with one current Issue linking to these controls. Resume retries explicitly. A model failure is not a media-file failure; unrelated classes continue. Unexpected whole-pass failures stop and report without an automatic retry loop.

## Independent maintenance lifecycles

OneCopy maintains separate ownership for:

1. Checking configured source folders.
2. Ingesting changes reported by filesystem watchers.
3. Completing missing information for known files.
4. Preparing presentation data required to use the library.
5. Generating enabled optional enrichment.

These lifecycles may coordinate through shared priority and durable pending work, but stopping or failing one does not silently redefine another. Every long-lived lifecycle exposes whether it is running, stopped or paused, complete, unavailable, or failed as applicable. An unexpected terminal failure leaves an explicit retry, resume, or repair path and is reported through the failure contract.

Normal browsing and file operations remain available while maintenance runs. A foreground file operation, section recheck, Settings apply, or index rebuild receives priority after background work reaches its next bounded safe point. Source checking, watcher ingestion, and file-information completion likewise take priority over automatic preparation and enrichment at that work's next safe point, except required preparation for the selected, visible, and nearby items, which interleaves with them at their own safe points. Automatic work interrupted this way restarts that unit later; an interrupted automatic transcription or video snapshot unit keeps no partial progress. Work the user requested, such as a Transcribe this file request or a preview being shown, does not yield to a foreground action unless that action changes the same file or rebuilds the index. Background discovery does not add later findings to an already confirmed file-operation plan.

## Checking configured sources

Checking source folders is a finite background pass over configured sources. It finds new paths, missing paths, and paths whose recorded size or modification time changed. `Check source folders after launch` defaults on and starts the pass only after Main is usable. Disabling that timing preference does not make reconciliation optional.

The pass does not block the usable main window. It compares inexpensive recorded filesystem facts first and does not reopen, rehash, or re-resolve a file whose recorded size and modification time are unchanged. A replacement that preserves both values may therefore remain unnoticed until another discovery mechanism or a library-index rebuild.

Background Work provides Start and Stop for checking source folders and shows its progress plus running, stopped, completed, or failed state. Stop takes effect at a safe checkpoint and preserves discoveries already recorded. The finite pass has no separate Pause state whose meaning duplicates Stop.

An explicitly requested source-folder check acknowledges successful completion even when it finds no changes and finishes immediately. Automatic launch checks stay quiet. A foreground action pauses the check in place, shown as waiting, and the check continues from where it stopped rather than walking its sources again; only an index rebuild or a Settings apply made while it waits makes it start over with the current configuration. The explicit request is retained through waiting and acknowledged only at its eventual completion; stopping or failing it does not produce success. Feedback never delays the work or claims the remaining preparation is finished.

A missing configured source or unavailable drive does not block the entire application. OneCopy continues with available copies, reports unavailable paths, and allows files to reappear when their source returns. Check again verifies source presence and recorded drive identity before requesting a source-folder check, whose existing completion restores watchers. An unreadable recorded identity remains blocked. Repeated clicks join the same request. Source-check and watcher-recovery completion refresh the source notice without launching another scan. A source can also be repaired through its configured root without restarting.

A drive that stops answering is treated as unavailable within a bounded time rather than waited on (`file-operations.md`, `Drives that stop answering`). A source check whose drive stops answering partway ends that source's pass incomplete with an Issue: nothing it did not see is marked missing, the source stays owed for the next check, and checking, watching, file-information completion, preparation, and file operations on other drives continue. Reading an original for its information or preparation fails as that one file when its drive does not answer.

A configured source that contains OneCopy's own data folder never indexes or reacts to that folder's contents: the app's index, logs, caches, and models are its own storage, never source content. This exclusion applies everywhere source discovery occurs — the source-folder check, watcher ingestion, and destination browsing — the same way deleted-file storage is excluded everywhere it occurs.

A macOS AppleDouble sidecar (`._name`) sitting beside its real file `name` in the same directory is operating-system metadata, not source content: macOS writes it to carry extended attributes and a resource fork on a volume that cannot store them natively, such as FAT, exFAT, or many network shares. It is excluded from discovery the same way, everywhere source discovery occurs, for as long as `name` exists beside it. An index row already recorded for one leaves the library the same way any other vanished path does, marked missing rather than raised as a failure. A `._name` file with no such sibling — its real file already gone, or its name unrelated — is ordinary content and remains discoverable.

## Watchers and section recheck

Filesystem watchers remain active while OneCopy is open. Each configured source is watched on its own, so a source whose drive does not answer when watching starts is reported without leaving the other sources unwatched. Watcher discoveries enter the same durable information-completion work as source-check discoveries. Lost watcher events and failed watcher passes trigger a source check limited to the affected configured roots when those roots are known; an unattributable path triggers a conservative check of all configured roots. Other sources retain their records. Recovery refreshes the library and leaves an actionable Issue with Check source folders when a root still cannot be checked. A stopped watcher remains an Issue even if that one recovery check succeeds. Ordinary watcher changes name affected sections; unchanged Main sections retain their loaded window, and bursts share a bounded refresh. Events without a reliable section scope refresh conservatively.

`Recheck this section`, also available through Cmd/Ctrl+R, rechecks the filesystem locations already represented by the open section, settles changed files, and reloads that section. It does not search unrelated source directories for files that might newly qualify for the section.

While all configured sources are already being checked, section recheck is unavailable with an explanation. A request that reaches the maintenance boundary despite that guard, or arrives while a file operation is running, returns busy with an explanation; it is never queued to run invisibly later. Section recheck, Settings apply, and index rebuild wait a bounded time for background work to reach its safe point. Section recheck and index rebuild otherwise return busy. Saved Settings never go unapplied: the index records the settings it was projected with, and a Settings apply that cannot start in time leaves the difference owed. Settings then says the change will apply once current work allows, Background Work shows file-information completion queued, and file-information completion applies the owed settings at its next turn, including after a restart, before completing other information; while that row is paused, the owed apply waits with it.

## Completing known file information

`Complete file information` consumes durable gaps for content identity, metadata, date evidence, and companion relationships. It is independent of source-folder checking and provides Pause and Resume. An unexpected terminal failure holds its queued work and shows as failed rather than paused, with Retry resuming it.

Stopping a source-folder check does not stop completion of information already discovered. Watcher-discovered work can also complete while the broader source check is stopped. Missing information does not disable unrelated library use; surfaces and operations use the facts currently known and remain truthful about what is unavailable.

An input-local identity-read or metadata-read failure settles that attempt without making the information complete. Later unrelated completion passes and ordinary browsing do not retry it. Restart, explicit recheck of its section, or an observed source change admits a new attempt; successfully completed information remains reusable. Absent or unsupported embedded metadata is an empty successful result, distinct from filesystem I/O failure. Failure receipts belong to file-information eligibility, independently of Issue visibility or dismissal.

## Required preparation

Inventory and file-information completion include hidden source copies under `library-visibility.md`. Presentation preparation and optional enrichment target review-eligible logical items; hiding an item retains its completed results rather than creating work to remove them.

Required work is work without which OneCopy's library or review surfaces are incomplete. Computational cost does not make it optional, and neither the first-launch wizard nor Settings provides a durable switch that disables it.

Required work comprises:

- Source discovery and reconciliation, content identity, metadata, date evidence, companions, and live folder watching.
- Image thumbnails and screen-sized image previews.
- Video posters and playable video preparation. A playable video whose container reports no duration still gets its poster and is eligible for transcription; it has no scene snapshots, which need a timeline, and it is never reported as a file to repair.
- Playable or otherwise supported preparation for Other files, including bounded truthful text or attributes when displayed.

Background Work may temporarily stop or pause required work so the user can release computer resources. A paused required lifecycle remains visibly incomplete and resumable; pausing does not convert it into a disabled feature.

## Optional enrichment and first launch

Optional enrichment adds useful analysis or navigation while leaving OneCopy functional when absent. The optional features are:

- Video scene snapshots.
- Similar-photo analysis.
- Advisory face scoring.
- Video transcription.
- Audio transcription.

Each optional feature has its own durable Settings choice and defaults on at first launch. Managed-tool installation state does not participate in that default because a fresh OneCopy installation is expected not to have downloaded those artifacts yet. Settings controls whether the feature is enabled; Background Work controls whether currently enabled work is temporarily paused.

Similar-photo analysis derives groups from current image facts and the chosen Stricter, Normal, or Looser grouping preset, without user-authored exclusions. Prior exclusion records and their backup history remain untouched and inert; they neither suppress grouping nor provide a current editing surface. Normal preserves the standard grouping thresholds; Stricter admits fewer pairs and Looser admits more. Face stars are shown whenever face scoring is enabled, without a separate display switch. Preview sizes, snapshot spacing, text-preview size limits, and the earliest believable photo year are internal policy; companion pairing is always enabled. A change in the grouping rules invalidates the affected derived groups without discarding unrelated indexed information.

Every supported desktop platform exposes the same optional feature set. Hardware acceleration is an implementation optimization, not a product capability boundary: an accelerator backend remains unsupported until its packaged correctness, cancellation, memory, responsiveness, fallback, and shutdown behavior pass physical acceptance, while the platform retains a reliable CPU path. In particular, Windows keeps face scoring and transcription available without claiming Vulkan, DirectML, CUDA, or another unaccepted accelerator.

Settings exposes acceleration per AI engine rather than one application-wide switch. The backend supplies a typed capability list for the current packaged binary and platform; the interface derives its choices from that list so a future Windows-only backend can be added without duplicating platform rules in the interface. `CPU only` is always available. Apple-silicon macOS additionally offers `Metal` for the shared transcription engine; face scoring has only the CPU path until another backend passes acceptance. Audio and video share the selected transcription acceleration because they share one engine, while their enablement remains independent.

The selected acceleration is durable and takes effect for new work after the current heavy operation reaches its safe boundary; changing it never requires recompilation or a separately built application. A distributed binary contains every backend it offers in its capability list. An absent setting uses the accepted platform default: Metal for transcription on Apple-silicon macOS and CPU-only otherwise. A saved backend that the current binary or platform does not offer is contained as an explicit configuration failure of that engine alone: its work shows as unavailable with the reason and a Settings action, while browsing, preparation, and every other engine continue. It never silently falls back or disables the feature. Resource-safety limits remain mandatory for every backend.

The first-launch wizard separates `OneCopy always prepares` from `Additional features`. The required section explains the unswitched identity, metadata, companion, thumbnail, preview, video-playback, and live-watching work. The additional-features section provides switches only for optional enrichment.

An ordinary missing managed tool never makes a wizard switch appear unavailable or start off. A concrete platform limitation may be explained without pretending that unavailable work is running or complete, while live storage and memory safety checks retain authority to pause admitted work at execution time.

An enabled optional feature whose required managed tool is unavailable remains enabled and visibly `Waiting for required tool`. It offers a direct Managed Tools action but never installs the tool implicitly. Windows face scoring requires both face models and OneCopy's pinned CPU runtime; it never loads an arbitrary system or search-path runtime. When the prerequisite becomes runnable, already-enabled work becomes eligible automatically; a feature that remained off stays off until the user enables it.

Managed-tool installation and update checking are bounded and cancellable, do not block normal browsing, and publish terminal state only for the operation that still owns the artifact.

When managed-tool launch checking is enabled, one app-wide UTC last-attempt timestamp controls its 24-hour network eligibility and is recorded immediately before every automatic or manual set-wide request sequence. Missing, invalid, future, or at-least-24-hour-old values are eligible. Failed checks preserve each tool's last-successful facts while the attempt guard still prevents repeated offline launches. This guard is independent from OneCopy's own release checker.

## Background Work controls

Background Work exposes distinct rows for work with distinct completion and policy:

| Work | Kind | Control |
|---|---|---|
| Check source folders | Required reconciliation | Start / Stop |
| Complete file information | Required information | Pause / Resume |
| Thumbnails, previews, and posters | Required presentation | Pause / Resume |
| Video snapshots | Optional enrichment | Pause / Resume |
| Similar-photo analysis | Optional enrichment | Pause / Resume |
| Face scoring | Optional enrichment | Pause / Resume |
| Video transcription | Optional enrichment | Pause / Resume |
| Audio transcription | Optional enrichment | Pause / Resume |

Video and audio transcription retain separate enabled settings, queue states, and controls. They may share transcription mechanics and storage without sharing user policy or becoming one combined queue surface. A transcript is kept in the library index with the model that produced it, the language the model detected when it reports one, and each segment's start and end in the media, so its text can be searched; it follows its content's identity and goes only when that content leaves the library.

Face scoring keeps every face it finds, with its box, confidence and expression, and which models checked each photo and how many faces they found, so a photo with no faces differs from one never checked; the face score is computed from them. Face results follow their content's identity and go only when that content leaves the library.

`Pause all` is a bulk action on file-information completion, preparation, and enrichment, not an overriding master state. Each row remains independently resumable afterward; resuming one leaves the others paused and never enables a feature disabled in Settings. Source checking retains its separate Start/Stop control, and watchers remain active. Pauses are temporary for the current app run.

Background Work owns controls, progress, temporary pauses, and prerequisites, not an Issues inbox. It shows no failed-output counts, and a class whose only remaining outputs failed is the one exception to linking Issues: its row and the status-bar segment state that some items failed and offer a way to open Issues, rather than the neutral no-running-work wording other settled work uses. Issues retains its separate status-bar entry and failure details. Runtime progress, preemption, and pause transitions never erase durable failure or prerequisite state or turn it into an unsupported claim of completion.

## Priority and resource use

When the keep-awake preference is enabled, actual source checking, information completion, preparation, and enrichment prevent idle system sleep. Queued, paused, stopped, failed, prerequisite-waiting, and otherwise idle work do not retain that assertion, including a source check or watcher batch waiting in place for other work. Overlapping execution shares one assertion until the last operation ends; later discoveries reacquire it when work begins. Disabling the preference or shutting down releases it even if work is still finishing. This does not keep the display lit or override an explicit system-sleep request. Failure to acquire the assertion is reported without stopping independent library work; switching the preference off and on retries it.

Maintenance continuously consumes runnable work after startup admission. User inactivity is never a prerequisite for background preparation or enrichment. Explicit pause, unavailable prerequisites, foreground exclusivity, and resource safety may prevent admission. Main, persistent Preview, the fullscreen view, and Comparison contribute to one current attention snapshot; hidden surfaces cannot replace the active workspace's priorities. Priority is:

1. Required work for the selected item, visible items, and a bounded region around the viewport.
2. Enabled optional enrichment supporting the active workspace's visible region.
3. Required work, then enabled optional work, moving outward through the active section in its displayed order.
4. The remaining library in bounded fair turns so no section or work class starves. Once urgent selected, visible, and nearby preparation is satisfied, a bounded share of turns reaches the library even while section work remains.

Changing the active workspace, section, displayed order, or viewport replaces stale pending priority hints. Comparison prioritizes its active card and current page across all displays without preparing original pixels or unseen comparison pages ahead. Required visible work runs beside unrelated transcription or analysis rather than waiting for it or stopping it, and a changed hint does not discard useful running work merely because its target moved offscreen. Work already at a bounded non-resumable step may reach its safe boundary before direction changes.

Required visible ordering is:

| Active content | Required order |
|---|---|
| Images | File identity and facts, then thumbnail, then screen preview |
| Videos | File identity and facts, then poster, then playable preview |
| Audio and Other files | File identity and facts, then playable or supported preview, then truthful text or attributes fallback |

Optional visible ordering is:

| Active content | Optional order |
|---|---|
| Images | Similarity facts and group refresh, then face scoring |
| Videos | Scene snapshots, then transcription |
| Audio | Transcription |
| Other non-audio files | No automatic optional enrichment |

Required visible preparation runs while the user is active and does not wait for a general idle timer. It runs in the preview lane beside automatic optional work instead of stopping it, and it interleaves with source checking, watcher ingestion, and file-information completion at their safe points: a check or watcher batch waits in place and continues afterwards, and completion ends its current turn and resumes its queued work. Preemption preserves completed results rather than presenting incomplete work as complete. A preempted transcript publishes no partial text and returns to its enabled queue. Moving among already prepared items does not by itself discard useful running work.

One coordinator owns derived-work admission, priority, cancellation, and publication. Preview preparation (thumbnails, screen previews, posters, and requested full-resolution images, which last only for the session), automatic or requested, has its own capacity-gated lane beside the one lane shared by snapshots, similarity, face scoring, and transcription, so neither a running transcription nor analysis stops or delays preview work, and preview work never stops them. A preview the user is waiting for waits a bounded time for a preview slot instead of failing because other media work is running. Independently checkpointed image thumbnail and screen-preview jobs may run concurrently when automatic CPU, decoded-memory, and subprocess budgets admit them. Concurrency leaves interactive headroom while the user is active, may use more capacity while quiet, and falls back as far as one job for large or uncertain decodes. Transcription has an explicit CPU budget and retains memory headroom during execution. Browsing and Comparison with prepared content remain usable while transcription runs. Exact worker, neighborhood, and batch sizes are implementation tuning rather than user settings.

Background Work refreshes durable debt when discovery, information completion, derived results, settings, or tools change it. Pending work remains visible before an executor starts, and queued, paused, waiting, failed, and complete remain distinct. A class whose only remaining outputs failed states that some items failed and points to Issues, instead of settling into the neutral no-running-work wording; the failed outputs stay durable and eligible at the next attempt boundary.

Restarting OneCopy and explicitly rechecking a section make its failed generated outputs eligible for a new attempt, including transcription. Merely reopening a section, selecting an item, or opening its preview does not retry a recorded failure. A failed face scoring or transcription is kept as a record, and the current model does not retry it until a boundary reopens it. Each boundary reopens existing failed outputs, not successful results or prerequisite-waiting states, and does not enable disabled enrichment or resume paused classes. Attempt eligibility is independent of Issue dismissal or retained diagnostic history. Rechecking a section scopes this reset to its current logical members, not every item sharing their folders; a failure from the new attempt remains settled until another explicit attempt boundary.

Database publication and user-visible state remain single-owned even when image conversion runs concurrently. Transcription, model-heavy analysis, snapshot extraction, and whole-library computation do not overlap one another unless measured platform evidence establishes safe memory use, cancellation, and responsiveness; the capacity-gated preview lane may run beside any one of them. No user setting can disable resource-safety limits or choose a raw thread count.

## Transcription generation

Supported videos with audio and supported audio files are eligible for transcription. Images and generic non-audio Other files are not. Video and audio have separate automatic-transcription settings, each enabled by default when runnable. The shared transcription engine uses the persisted platform-aware acceleration choice described above. Apple-silicon macOS defaults to its accepted Metal backend but can be switched to CPU-only; Windows currently offers and defaults to the portable CPU backend until another packaged accelerator independently passes the release bar.

Transcription detects spoken language automatically. OneCopy does not add a language selector or translation mode to this lifecycle. A completed attempt that finds no speech records `Checked — no speech found` as a successful empty result rather than remaining pending or failed.

A transcript belongs to the content identity, so byte-identical copies share one result. Changed bytes create a new identity and new transcription work.

Video and audio queues share one coordinated heavy transcription engine and receive fair turns so neither medium starves the other. The coordinator never starts a second engine for a manual request.

When automatic transcription is enabled for a medium, a pending selected item is prioritized automatically rather than receiving a redundant Transcribe command. When automatic transcription is disabled for that medium, `Transcribe this file` submits an intentional one-off request. A one-off request made while another transcript is running enters the same coordinator as the next manual-priority job. Failed work remains settled until an explicit attempt boundary; the Issues inbox does not own its retry controls.

Pause and preemption by foreground actions or index upkeep take effect at the transcription engine's safe cancellation boundary. Partial text is never published as complete. Preempted automatic work returns to its queue.

Cancelling a first attempt returns the content to Not transcribed. Cancelling replacement work preserves the prior completed transcript. Completed transcripts are not redone merely because a newer model becomes available.

Completed transcripts remain reconstructible derived information while their content identity exists. `Re-transcribe` keeps the completed result available and replaces it only after the new result succeeds; a failed or cancelled replacement leaves the previous transcript intact and reports the unsuccessful attempt.

## Rebuilding the library index

`Rebuild library index…` is a Settings maintenance action, not an everyday refresh command. It always discards the library index so it can be derived again, and closes the open Issues; a confirmation dialog offers three further, independent choices — discard previews and posters, discard face scores, and discard transcripts — each unchecked (kept) by default, since none is wrong to keep and each costs time to regenerate for no different a result with the same model and files. The transcript choice carries a note saying so. Transcripts and face scores survive a rebuild unless their choice says to discard them. Preparation stored under a provisional identity names a path rather than content, never a real one, so it is always discarded regardless of these choices; preparation stored under an exact content identity is discarded only when its choice says to, and is otherwise reused. One owner (the index-clearing command) decides what a rebuild clears; anything discarded is regenerated automatically by the same background work that derives it the first time.

Rebuilding never changes user files, Settings, managed tools, or retained authored records. It cannot overlap an active file operation because it removes information used to plan that operation. A rebuild request made while mutation work is active is refused with an explanation rather than queued for later execution.
