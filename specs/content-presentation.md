# Content Presentation

## Truthful body selection

- A displayed file receives the richest truthful built-in body OneCopy can provide: supported image, video, audio, or another specialized presentation; bounded text only when the bytes are convincingly textual; otherwise attributes.
- Presentation capability never changes the logical item's library section. Audio remains in Other files, and a fallback body does not reclassify an item.
- A specialized decoder failure remains visible. Binary-looking data is not forced through a permissive legacy text decoder merely to avoid showing attributes.
- Moving from a loading or attributes body to a later-prepared richer body for the same live item does not change Main selection, transient sequence, focus, or file-operation availability.
- Filename, logical identity, known facts, and rendered content change as one truthful package. Rapid navigation may skip abandoned work but never shows old pixels or playback under a new item's name.

## Image presentation and inspection

- Main, similar-image, video-scene, and inner drag-payload thumbnails contain the complete media without cropping and keep media, selection, focus, and drag treatment coherent within their bounds. Hold inspection does not apply to thumbnails because dragging owns that gesture.
- Persistent Preview, Quick View, true fullscreen, and Comparison contain the complete image without cropping.
- `Enlarge small images in Preview` and `Enlarge small images in Quick View` are independent settings and default on. Each enlarges a small image only to the largest contained size; turning it off caps ordinary presentation at the image's original dimensions.
- Click-and-hold in every larger image presentation temporarily shows original pixels at 1:1 under the pointer. Dragging inspects other source regions. A smaller original remains centered at real size.
- Release, pointer loss, blur, item change, placement change, presentation switch, or close ends inspection and restores the fitted image. The hold never also clicks, double-clicks, drags a file, or changes selection.
- Failure to load original pixels leaves a working fitted image intact and reports the inspection failure without closing the owning view.
- Ordinary click, double-click, and Enter have no fitted-image body action. Transient-viewer wheel behavior belongs to `viewing-sessions.md`.

## Shared playback ownership

- Video and audio are separate presentation bodies with separate policy. They may share source loading, play/pause, seek, position, volume, app-wide Sound, handoff, error, and safe-release primitives without becoming modes of one universal player.
- `Video autoplay`, `Audio autoplay`, and app-wide `Sound` are independent settings, all default on, and are directly available in Main as well as Settings.
- Autoplay applies only when a genuinely new media item is shown. Changing it does not alter current playback. Rapid navigation cancels automatic starts for abandoned items.
- App-wide Sound applies immediately to every OneCopy-owned player without changing position or playing state. One remembered nonzero volume value is shared; Sound off silences playback without replacing it, and Sound on restores it.
- Only one OneCopy surface owns playback at a time. Moving the same logical media item among Preview pane, Preview window, Quick View, and fullscreen preserves the latest position as closely as the media permits together with playing/paused state, without restart, duplicate sound, or autoplay reapplication.
- Persistent Preview remains open but suspended while the transient viewer owns the same session. Returning transfers the latest state back when Preview remains open; otherwise the session ends.
- Moving to another logical item ends the prior session. Returning later starts at the beginning and reapplies that medium's autoplay setting. OneCopy keeps no durable playback-position history per item.
- Natural completion stops at the end without looping, restarting, selecting another library item, or playlist-style advance.
- Before a file operation or external delegation, OneCopy pauses and releases its own media readers. A failed file operation restores the same surviving item's prior live position and playing state when possible and reports restoration failure otherwise; a successful operation follows ordinary selection and sequence recovery.

## Video body

- Main video tiles show a static uncropped poster, duration, and truthful preparation or failure state. They never animate, play, or produce sound on hover, selection, or navigation.
- Preview and transient video presentations contain the complete picture without cropping and provide play/pause, seek, elapsed/duration, volume, app-wide Sound, and a usable poster when playback is unavailable.
- Ordinary click on the video picture and Enter when the player is visible toggle play/pause. Actual controls retain their accessible actions. Space remains a viewing-session transition, not a playback command.
- Double-click on the video picture has no special action.
- Click-and-hold pauses and shows the current frame at original video-pixel size for drag inspection, then resumes only if it had been playing. Controls, transcript, scene strip, and scrollbars never start inspection.
- Activating a scene snapshot seeks to its timestamp and begins playback. There is no separate autoplay-after-snapshot setting.
- Unsupported or failed playback retains poster, attributes, navigation, deletion, and Open in Default App.

## Audio body

- Audio uses a distinct body with filename, play/pause, seek, elapsed/duration, app-wide volume and Sound, useful attributes, and transcript when available. It does not inherit a video canvas, scene strip, frame inspection, or video layout.
- Enter toggles playback when the audio player is visible. Ordinary surface click and double-click have no invented audio action outside real controls.
- Audio follows the shared live-session, autoplay, natural-completion, external-delegation, and safe-release rules without becoming a video mode.

## Text body

- Text preview is built in and cannot be disabled. The configurable whole-file eligibility limit defaults to 2 MiB and remains positive. A larger file shows attributes and the reason without partial preview or Load more.
- Automatic decoding checks a Unicode marker, exact UTF-8 validity, strong binary evidence, a maintained encoding detector, then the configured fallback, which defaults to UTF-8.
- The encoding picker begins with Automatic and exposes every canonical decoder the current runtime can reliably support. Aliases aid search without creating duplicate choices. Automatic identifies whether it used a Unicode marker, exact UTF-8, detected encoding, or the configured fallback. A manual choice re-decodes immediately and follows that byte-identical content among Preview and transient presentations for the current app session.
- UTF-32LE and UTF-32BE are available for explicit selection and are chosen automatically only when a Unicode marker makes the result certain.
- Text renders as inert read-only selectable plain text, preserves line breaks, and never executes markup. Syntax highlighting, editing, saving, and permanent per-file encoding records are outside this lifecycle.
- Wrap is on by default and shared across Preview and transient presentations. Turning it off permits horizontal scrolling.
- A focused text body supports pointer selection, word selection, Select All, Copy, and document scrolling. Delete and Backspace retain the owning view's file meaning because the text is read-only; only a genuinely editable control consumes them for editing. In a mixed transient sequence, Left/Right still changes files; horizontal document movement uses its scrollbar or horizontal gesture.
- Leaving the item discards text selection and scroll while preserving its session encoding choice. Watcher-confirmed content change reloads bounded bytes and discards stale text state.
- Invalid byte sequences under a selected fallback render replacement characters rather than crashing or silently discarding bytes. Decode failure explains the problem and preserves useful alternative encodings. Content that is not convincingly textual falls back to attributes.

## Attributes body

- Attributes are the truthful fallback for binary-looking data, over-limit text, unsupported or failed specialized formats, unavailable content, or any item with no richer body.
- The body shows representative filename and path, known type, size, relevant dates, exact-copy count, accessible physical-copy paths, and the reason no richer presentation is shown. Unknown values remain unknown.
- Failure to read one attribute never blanks the complete body.
- Ordinary click, double-click, Enter, and hold have no attributes-specific action. The body may scroll and retains Open in Default App plus per-copy Reveal in File Manager.

## Transcript presentation

- Supported video and audio may show the transcript supplied for their current logical content identity. Changed bytes never display an old transcript as current. Generation, cancellation, replacement publication, and cache ownership belong to `library-maintenance.md`; this contract owns only the rendered transcript state and controls.
- Transcript text is read-only, selectable, copyable, and divided into timestamped segments. It appears as a subordinate collapsible panel and never obscures essential controls or makes video unusably small.
- Video and audio remember transcript open/collapsed state independently. Preview placement and transient-presentation switches retain panel state, scroll, and text selection for the same live media session. Restart persistence of that preference is not specified.
- Pending, queued, running, paused, disabled, failed, and replacement states are explicit. Incomplete generated words are never presented as a completed transcript.
- When automatic transcription for the medium is enabled, pending media shows its queued state without a redundant primary Transcribe action; running media shows progress and access to Background Work. When automatic transcription is off, `Transcribe this file` is available. Failure offers Retry and access to Issues. Completed content offers Re-transcribe only as a secondary action. Queueing and replacement publication follow `library-maintenance.md`.
- Activating a timestamp seeks the current OneCopy playback session and begins playback. Ordinary transcript text selection never seeks. A transcript creates no second player.
- A focused transcript uses Up/Down and Page Up/Page Down for scrolling. Enter on a focused timestamp seeks and plays. Space, `F`, Escape, and read-only Delete/Backspace retain the owning view's meanings unless a genuinely editable control owns the key.
- Re-transcription leaves the prior completed transcript visible until the replacement succeeds. Failure or cancellation preserves the prior result and reports the new attempt.
- OneCopy adds no transcript-only search. When ordinary library search is designed, completed transcript text participates alongside other searchable facts under that future contract.

## External delegation and physical copies

- Open in Default App is one explicit action for images, video, audio, text, attributes, and Comparison cards. A completed logical item opens its deterministic representative copy through an app-resolved known identity. An individually known file whose content identity is not complete may open through its valid indexed path. The interface never supplies an unrestricted path.
- OneCopy pauses its audio or video before delegation and remains paused when focus returns. The external application owns an independent session; OneCopy does not synchronize position, volume, Sound, edits, completion, or close state.
- Watchers discover external edits, renames, replacements, and deletion through ordinary reconciliation. There is no external-editor transaction.
- Failure to launch the external application is reported through the notification and Issues system without changing OneCopy selection or viewer state.
- Reveal in File Manager remains separate because one logical item may have several physical copies and the user may need to choose one. Trash Reveal supports manual recovery under `file-operations.md`.
