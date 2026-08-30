# Library Maintenance

This contract defines how OneCopy discovers source changes, completes known file information, prepares required presentation data, schedules optional enrichment, generates transcripts, exposes Background Work controls, and rebuilds reconstructible library state. Logical-item calculation is owned by `library-items.md`; completed-content presentation is owned by `content-presentation.md`; general failure presentation is owned by `failures-and-recovery.md`.

## Independent maintenance lifecycles

OneCopy maintains separate ownership for:

1. Checking configured source folders.
2. Ingesting changes reported by filesystem watchers.
3. Completing missing information for known files.
4. Preparing presentation data required to use the library.
5. Generating enabled optional enrichment.

These lifecycles may coordinate through shared priority and durable pending work, but stopping or failing one does not silently redefine another. Every long-lived lifecycle exposes whether it is running, stopped or paused, complete, unavailable, or failed as applicable. An unexpected terminal failure leaves an explicit retry, resume, or repair path and is reported through the failure contract.

Normal browsing and file operations remain available while maintenance runs. A foreground file operation receives priority after background work reaches its next bounded safe point. Background discovery does not add later findings to an already confirmed file-operation plan.

## Checking configured sources

Checking source folders is a finite background pass over configured sources. It finds new paths, missing paths, and paths whose recorded size or modification time changed. A configured setting controls whether this pass starts after launch; disabling that timing preference does not make reconciliation optional.

The pass does not block the usable main window. It compares inexpensive recorded filesystem facts first and does not reopen, rehash, or re-resolve a file whose recorded size and modification time are unchanged. A replacement that preserves both values may therefore remain unnoticed until another discovery mechanism or a library-index rebuild.

Background Work provides Start and Stop for checking source folders. Stop takes effect at a safe checkpoint and preserves discoveries already recorded. The finite pass has no separate Pause state whose meaning duplicates Stop.

A missing configured source or unavailable drive does not block the entire application. OneCopy continues with available copies, reports unavailable paths, and allows files to reappear when their source returns.

## Watchers and section recheck

Filesystem watchers remain active while OneCopy is open. Watcher discoveries enter the same durable information-completion work as source-check discoveries. A watcher failure becomes visible rather than silently leaving the library stale.

`Recheck this section`, also available through Cmd/Ctrl+R, rechecks the filesystem locations already represented by the open section, settles changed files, and reloads that section. It does not search unrelated source directories for files that might newly qualify for the section.

While all configured sources are already being checked, section recheck is unavailable with an explanation. A request that reaches the maintenance boundary despite that guard returns busy; it is never queued to run invisibly later.

## Completing known file information

`Complete file information` consumes durable gaps for content identity, metadata, date evidence, and companion relationships. It is independent of source-folder checking and provides Pause and Resume.

Stopping a source-folder check does not stop completion of information already discovered. Watcher-discovered work can also complete while the broader source check is stopped. Missing information does not disable unrelated library use; surfaces and operations use the facts currently known and remain truthful about what is unavailable.

## Required preparation

Required work is work without which OneCopy's library or review surfaces are incomplete. Computational cost does not make it optional, and neither the first-launch wizard nor Settings provides a durable switch that disables it.

Required work comprises:

- Source discovery and reconciliation, content identity, metadata, date evidence, companions, and live folder watching.
- Image thumbnails and screen-sized image previews.
- Video posters and playable video preparation.
- Playable or otherwise supported preparation for Other files, including bounded truthful text or attributes when displayed.

Background Work may temporarily stop or pause required work so the user can release computer resources. A paused required lifecycle remains visibly incomplete and resumable; pausing does not convert it into a disabled feature.

## Optional enrichment and first launch

Optional enrichment adds useful analysis or navigation while leaving OneCopy functional when absent. The optional features are:

- Video scene snapshots.
- Similar-photo analysis.
- Advisory face scoring.
- Video transcription.
- Audio transcription.

Each optional feature has its own durable Settings choice and defaults on when runnable. Settings controls whether the feature is enabled; Background Work controls whether currently enabled work is temporarily paused.

The first-launch wizard separates `OneCopy always prepares` from `Additional features`. The required section explains the unswitched identity, metadata, companion, thumbnail, preview, video-playback, and live-watching work. The additional-features section provides switches only for optional enrichment.

When a concrete prerequisite or enforced storage or memory safety check makes an optional feature unavailable, its wizard switch starts off and explains the condition. The user may still choose the feature, but unavailability is never represented as running or completed work, and the application retains its error-containment and resource-safety boundaries.

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

Video and audio transcription retain separate enabled settings, queue states, and controls. They may share transcription mechanics and cache storage without sharing user policy or becoming one combined queue surface.

## Priority and resource use

Maintenance follows the user's current location instead of draining a fixed media-type backlog. Priority is:

1. Required work for the selected item, visible items, and a bounded region around the viewport.
2. Enabled optional enrichment for that same visible region.
3. Required and enabled optional work moving outward through the active section.
4. The remaining library in bounded fair turns so no section or work class starves.

Changing the active section, displayed order, or viewport replaces stale pending priority hints. Work already at a bounded non-resumable step may reach its safe boundary before direction changes.

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

Required visible preparation runs while the user is active and does not wait for a general idle timer. It may preempt automatic optional work at that work's next safe cancellation point. Preemption preserves completed results rather than presenting incomplete work as complete. A preempted transcript publishes no partial text and returns to its enabled queue. Moving among already prepared items does not by itself discard useful running work.

Only one CPU-heavy derived operation runs at a time. Transcription, snapshot extraction, face scoring, and large preview conversion do not compete concurrently. Exact neighborhood and batch sizes remain implementation tuning rather than user settings.

## Transcription generation

Supported videos with audio and supported audio files are eligible for transcription. Images and generic non-audio Other files are not. Video and audio have separate automatic-transcription settings, each enabled by default when runnable.

A transcript belongs to the content identity, so byte-identical copies share one result. Changed bytes create a new identity and new transcription work.

Video and audio queues share one coordinated heavy transcription engine and receive fair turns so neither medium starves the other. The coordinator never starts a second engine for a manual request.

When automatic transcription is enabled for a medium, a pending selected item is prioritized automatically rather than receiving a redundant Transcribe command. When automatic transcription is disabled for that medium, `Transcribe this file` submits an intentional one-off request. A one-off request made while another transcript is running enters the same coordinator as the next manual-priority job. Failed work does not retry without limit and provides Retry.

Pause and required-work preemption take effect at the transcription engine's safe cancellation boundary. Partial text is never published as complete. Preempted automatic work returns to its queue.

Completed transcripts remain reconstructible derived information while their content identity exists. `Re-transcribe` keeps the completed result available and replaces it only after the new result succeeds; a failed replacement leaves the previous transcript intact and reports the failure.

## Rebuilding the library index

`Rebuild library index…` is a Settings maintenance action, not an everyday refresh command. It discards reconstructible library information, generated preparation, transcripts, and Issues so they can be derived again.

Rebuilding never changes user files, Settings, managed tools, or user-authored choices such as exclusions from similarity groups. It cannot overlap an active file operation because it removes information used to plan that operation.
