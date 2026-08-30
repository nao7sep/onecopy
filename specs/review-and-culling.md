# Review and Culling

## Shared vocabulary and ownership

- A section is one item in the main window's left menu. Main owns each section's displayed order, file selection, one anchor, and resumable work position.
- The anchor is the one logical item supplying Details, persistent Preview, keyboard position, transient-viewer start, and resumable position. The selection is the complete set a Main operation may affect. They are not interchangeable.
- Details belongs to Main: it presents the anchor's facts and actions and owns no independent selection, sequence, or playback session.
- Persistent Preview is one follower with pane and separate-window placements. It never owns another library selection.
- Quick View and true fullscreen are two presentations of one temporary viewer session. They share their frozen sequence, current item, prepared content, deletion state, playback position, transcript state, failures, and Main-anchor updates.
- Comparison is a separate temporary decision workspace for one live similar-image group. It owns a page-local draft selection whose meaning is applied only by an explicit Comparison action.

## Main selection and resumable position

- Opening a nonempty section for the first time selects its first displayed item. Returning during the same session restores that section's last anchor as an exclusive selection rather than restoring an old multi-selection.
- Restart restores the last-open section and its anchor, not a separate active selection for every section. Every restored, recovered, clicked, or keyboard-selected anchor is scrolled into view.
- When an anchor disappears, recovery chooses the next surviving item in the prior displayed order, then the previous, then none. During multi-selection recovery, surviving selected items remain selected and the next then previous surviving selected neighbor becomes anchor before an unselected neighbor.
- Restoration and recovery show surrounding context near the viewport center. Ordinary keyboard navigation scrolls only far enough to keep the anchor visible.
- Opening a section from the sidebar leaves keyboard focus in the sidebar. Right Arrow or Tab enters the grid/list; item commands do not act merely because section entry automatically selected an item.
- Ordinary click selects one item exclusively and collapses a multi-selection. Clicking the only selected item leaves it selected. Empty-space click clears selection and anchor.
- Cmd/Ctrl-click toggles one item without clearing other selected items; a newly added item becomes anchor. Removing the anchor chooses the next selected item in displayed order, then the previous selected item, then none.
- In image and video grids, Shift-click and Shift-navigation extend or shrink one uninterrupted range from the anchor and replace the prior selection. In the Other-file list, they preserve deliberately toggled rows outside the range.
- Arrow, Page Up/Page Down, Home, and End navigation clamps at displayed bounds and never wraps. Image/video grids use spatial arrows; the Other-file list uses Up/Down and reserves Left/Right.
- Sorting, column sorting, refresh, watcher changes, and improved preview information preserve selection and anchor by logical identity and keep the anchor visible.
- Dragging an unselected item selects and drags it. Crossing the drag threshold on an already selected member preserves and drags the selection; release without crossing the threshold remains an ordinary click.

## Main transitions

| Action | Image | Video | Audio in Other files | Text or attributes in Other files |
|---|---|---|---|---|
| Space | Open Quick View from the current selection | Open Quick View from the current selection | Open Quick View from the current selection | Open Quick View from the current selection |
| F | Open the same transient session directly in true fullscreen | Open the same transient session directly in true fullscreen | Open the same transient session directly in true fullscreen | Open the same transient session directly in true fullscreen |
| Double-click | Exclusively select the clicked item, then open Quick View under the same whole-section sequence rule as Space with one selection | Exclusively select the clicked item, then open Quick View under the same whole-section sequence rule as Space with one selection | Exclusively select the clicked item, then open Quick View under the same whole-section sequence rule as Space with one selection | Exclusively select the clicked item, then open Quick View under the same whole-section sequence rule as Space with one selection |
| Enter | Open Comparison only when the complete selection belongs to one live similar-image group | Toggle the visible persistent player's play/pause state | Toggle the visible persistent player's play/pause state | No application action |

- With no selection, Space or F leaves Main in place and explains that an item must be selected.
- With one selection, Quick View/fullscreen freezes the complete current section in displayed order and starts at the anchor. With several selected items, it freezes only that selected subset in displayed order and starts at the anchor.
- Main Enter never silently chooses the anchor's similar-image group from a mixed or ungrouped image selection. It stays in Main and explains that Comparison requires one live similar group.
- Video/audio Enter does nothing when a playable persistent Preview is not visible. It never opens, closes, raises, or focuses Preview or Quick View merely to create a playback target.
- Open in Default App is an explicit labeled action for every content type and is never hidden behind double-click. The backend resolves the deterministic current representative instead of trusting an interface-supplied path. Reveal in File Manager remains the separate action for choosing a physical copy.
- Main Delete/Backspace Trashes the complete selection. Confirmed Shift+Delete permanently deletes the complete selection.

## Persistent Preview

- Preview remains armed when no item is selected and shows an explicit no-selection state. Opening, closing, or moving Preview never changes Main selection.
- Pane and separate window are placements of the same follower. Changing placement leaves no duplicate and preserves the current live image, playback, transcript, text scroll, session encoding, wrap state, and expanded attributes as applicable.
- Preview's open/closed state and placement survive restart. The separate window remembers its display, position, size, and maximized state; its first opening is maximized but never true fullscreen. Media position, text scroll, and transient control focus do not survive restart.
- Main anchor changes replace pixels/content, filename, identity, and controls as one truthful package. A multi-selection shows its anchor plus the selected count; failures never leave an old item's pixels under a new name.
- Explicitly showing the separate window brings it forward without taking keyboard focus from the Main grid and never makes it permanently always-on-top. Its footer shows the filename, selected count when applicable, and the hold-to-inspect hint.
- The pane does not become another item-navigation context. When no body control owns input, the focused separate Preview window forwards Arrow/Page/Home/End navigation, Shift-range navigation, the content-specific Enter action, Delete/Backspace, and confirmed Shift+Delete to Main.
- In separate Preview, Space has no view action, Escape closes Preview, and F invokes the app-level fullscreen session from Main's current selection. Ordinary and double-click have no Preview-level image action and never provide a hidden fullscreen gesture.
- Delete/Backspace in Preview Trashes the complete Main selection for every body, including read-only text. Confirmed Shift+Delete permanently deletes it. Only a genuinely editable field consumes deletion keys for editing.
- Closing Preview stops following without changing selection. It ends an owned audio/video session unless Quick View/fullscreen already owns that same session; reopening later begins at the start and reapplies the medium's autoplay setting. A failed separate window offers pane placement; any failed body retains filename, known attributes, navigation, file actions, and a visible explanation/retry or external-open path when meaningful.

## Image presentation

- Main and similar-image thumbnails clip media cleanly inside their bounds. Rounded containers never expose gaps around a square image.
- Preview, Quick View, fullscreen, and Comparison contain the complete image without cropping. `Enlarge small images in Preview` and `Enlarge small images in Quick View` are independent, default-on preferences.
- Click-and-hold in every larger image presentation temporarily shows original pixels at 1:1 under the pointer. Drag inspects other areas; release or cancellation restores fit and does not also click, double-click, or change selection.
- Failure to load original pixels preserves an already working fitted image. Pointer loss, blur, view change, and close cancel inspection and restore that fitted state.
- Quick View image navigation uses Left/Right and Page Up/Page Down for previous/next, Home/End for sequence bounds, and leaves Up/Down unused. Navigation never wraps.
- Ordinary click, double-click, Enter, and wheel/trackpad scrolling have no fitted-image Quick View action.

## Video presentation and playback

- Video thumbnails show a static uncropped poster, duration, and truthful preparation or failure state. They never animate, play, or produce sound on hover, selection, or navigation.
- Video Preview and the transient viewer contain the complete picture without cropping and provide play/pause, seek, elapsed/duration, app-wide volume, mute, and a usable poster when playback is unavailable.
- One continuous video playback session moves among Preview pane, Preview window, Quick View, and fullscreen for the same video. Handoff preserves exact position and playing/paused state without reapplying autoplay or flashing an intermediate poster.
- A genuinely new video begins at the start and applies Video autoplay. Rapid navigation cancels pending automatic starts. Leaving the video ends its session; returning later begins from the start.
- While Quick View/fullscreen owns playback, persistent Preview remains open but suspended. Returning transfers the current position and state back when Preview is open; otherwise it ends the session.
- Ordinary click on the video picture and Enter toggle play/pause. Actual controls keep their accessible actions. Space remains a view transition and never controls playback.
- Click-and-hold pauses and shows the current frame at original video-pixel size for drag inspection, then resumes only if it had been playing. Double-click has no video-view action.
- Scene-snapshot activation explicitly seeks to its timestamp and begins playback. There is no separate autoplay-after-snapshot preference.
- Natural completion stops at the end. OneCopy neither loops nor automatically advances to another library item.
- Unsupported or failed playback retains poster, attributes, navigation, deletion, and Open in Default App. External open pauses OneCopy playback and leaves external playback independent and unsynchronized.

## Audio presentation and playback

- An audio item receives a distinct presentation with filename, play/pause, seek, elapsed/duration, app-wide volume, mute, known attributes, and transcript when available. It does not inherit a video canvas, scene strip, or video-only policy.
- One audio playback session moves among Preview pane, Preview window, Quick View, and fullscreen for the same audio item, preserving position and playing/paused state. Moving to another item ends it; returning later begins at the start and reapplies Audio autoplay.
- Enter toggles play/pause when the audio player is visible. Ordinary surface click and double-click have no invented audio action outside real controls.
- Natural completion stops at the end without looping or moving to another item. External open pauses OneCopy and remains independent.

## App-wide playback state

- Video autoplay, Audio autoplay, and app-wide Sound default on and are always-visible Main status-bar controls mirrored in Settings; state changes apply to every open OneCopy window. Autoplay changes affect future media changes, not the current playing state.
- App-wide Sound applies immediately to every OneCopy-owned player without changing position, playing state, or autoplay. One remembered volume value is shared; Sound off forces silence and Sound on restores the remembered nonzero volume.
- Native player mute/volume changes update the same shared state. No embedded or platform player may silently contradict app-wide Sound.
- Audio and video may share small source, playback, seek, volume, handoff, error, and mutation-release primitives. They remain separate preview bodies and policy owners rather than modes of one universal player.
- Before a file operation targets media OneCopy is playing or holding open, OneCopy pauses that session and releases its readers. If the operation fails and the same logical item survives, its prior position and playing/paused state return; success follows Main's ordinary recovery.

## Text and attributes presentation

- Specialized image/video/audio presentation is tried before text. A specialized decoder failure remains visible and falls back to bounded text only when the bytes are convincingly textual; otherwise it falls back to attributes. A fallback replaces only the presentation body and does not disturb Main selection.
- Text preview is always available and cannot be disabled. The whole-file eligibility cap is configurable and defaults to 2 MiB; a larger file shows attributes and the reason, with no incremental Load more path.
- Automatic decoding order is Unicode marker, exact UTF-8 validation, binary guard, maintained encoding detection, then the configured fallback, UTF-8 by default. Binary-looking data is not forced through a permissive legacy decoder.
- The encoding picker begins with Automatic and exposes every canonical decoder the current runtime can reliably support; aliases aid search without appearing as duplicate choices. A manual choice re-decodes immediately and follows that content across Preview/Quick View/fullscreen for the current app session.
- UTF-32LE and UTF-32BE are available for explicit selection and are chosen automatically only when a Unicode marker makes the encoding certain.
- Text is read-only and selectable, preserves line breaks, and wraps by default. Wrap text is shared across Preview and transient views; disabling it permits horizontal scrolling.
- Text renders as inert plain text rather than markup or executable content; syntax highlighting is not part of this lifecycle. Automatic decoding identifies whether its result came from a Unicode marker, exact UTF-8, uncertain detection, or configured fallback, and invalid fallback sequences render replacement characters rather than failing the whole preview.
- A focused text body owns text selection, copy, and document scrolling. In Quick View/fullscreen, app-level Space, F, Escape, and current-file deletion remain authoritative; Page Up/Page Down scroll the document rather than changing files while text owns focus.
- Leaving a text item discards its text selection and scroll position while preserving its session-only content encoding choice. Watcher-confirmed content change reloads the bounded bytes and discards stale text selection.
- Attributes-only presentation shows the representative filename/path, known type, size, relevant dates, exact-copy count, accessible per-copy paths, and the truthful reason no richer body is shown. Unknown values remain unknown.
- Attributes provide explicit Open in Default App and Reveal in File Manager; Reveal lets the user choose a physical copy. Failure to read one attribute never blanks or crashes the complete fallback surface.

## Transcript presentation

- A completed transcript belongs to byte-identical logical content and is shared by exact copies. Changed content never displays the old transcript as though it still applies.
- The transcript is read-only selectable text divided into timestamped segments and is available in persistent Preview, Quick View, and fullscreen. Moving the same media session carries its transcript and visibility state without reloading.
- The transcript is a subordinate collapsible panel that never obscures controls or makes video unusably small. Its open/collapsed state follows the current media session across Preview, Quick View, and fullscreen without becoming another durable setting.
- Activating a timestamp seeks the current player to that segment and begins playback. Clicking ordinary transcript text selects text without seeking.
- Pending, running, paused, disabled, failed, and successful-no-speech states are explicit and link to the owning Background Work, Resume, managed-install, Transcribe, or Retry path as appropriate. Incomplete generated words are not displayed as a completed transcript.
- Re-transcription keeps the prior transcript visible as Updating and atomically replaces it only after complete success. Failure or cancellation retains the prior transcript and records the new attempt's Issue.
- OneCopy does not add transcript-only search. When ordinary global library search is designed, completed transcript text participates alongside filenames and metadata; query and result design belongs to that future lifecycle.

## Quick View and true fullscreen

- Quick View is a temporary Main overlay. True fullscreen is the same session rendered through one dedicated reusable borderless window on the invoking display, covering OneCopy chrome and the operating-system menu bar, Dock, or taskbar.
- macOS uses non-Spaces simple fullscreen behavior. OneCopy never moves Main or Preview into macOS Spaces fullscreen, and embedded media never starts an independent element, browser, Tauri, or platform fullscreen mode.
- From Main, Space opens Quick View and F opens fullscreen. In Quick View, Space returns to Main and F switches to fullscreen. In fullscreen, Space switches to Quick View, F returns to Main, and Escape from either presentation returns to Main.
- Presentation switches preserve sequence, current item, Main anchor, selection, prepared content, playback, inspection, transcript, notifications, and failures without reload.
- The transient viewer shows the current filename, sequence position such as `3 of 18`, visible previous/next controls, and Close through compact overlaid chrome that may hide during inactivity rather than permanently reserving thick bars.
- Closing returns focus to Main at the current visible anchor. The transient session never restores after restart.
- Image/video transient navigation uses Left/Right or Page Up/Page Down for one previous/next item and Home/End for bounds. In mixed Other-file sequences, Left/Right always changes the item, including for unwrapped text; horizontal text movement uses its scrollbar or a horizontal gesture. Audio uses Page Up/Page Down and Home/End for sequence navigation, while focused text and attributes use Up/Down for ordinary scrolling, Page Up/Page Down for page scrolling, and Home/End for document bounds. Navigation never wraps.
- Whole-section navigation makes the viewed item Main's exclusive selection and anchor. Selected-subset navigation preserves the selected set and moves only its anchor.
- Delete/Backspace in Quick View/fullscreen Trashes only the displayed logical item, never the hidden Main multi-selection. Confirmed Shift+Delete permanently deletes only that item. Success removes it from sequence and selection, then chooses next, previous, or closes.
- External disappearance uses the same next, previous, then close recovery. Failure preserves the current sequence when possible and remains visible through notifications and Issues.
- Fullscreen enter/leave operations are serialized. Every close, error, and shutdown path leaves presentation mode, clears temporary topmost state, and restores Main so the Dock/taskbar cannot remain suppressed.

## Comparison

- Main Enter or the visible Compare action opens Comparison only when the complete selection belongs to one live similar-image group. Comparison loads that complete group; fewer than two live comparable members leaves Main in place with an explanation.
- Membership is frozen at entry. Newly found or automatically regrouped images wait for the next session; missing images and deliberate Not similar decisions update the active session. Unfinished sessions never restore after restart.
- After Comparison closes, Persistent Preview returns to its previous pane/window placement and follows Main's recovered anchor.
- A similar group has no size limit. `Maximum images in Comparison` limits only simultaneous display, defaults to 16, has a minimum of two, and has no product-level upper limit.
- Actual page size is the minimum of the configured cap, remaining members, and the number the eligible configured displays can show legibly. OneCopy uses as many displays as needed rather than imposing a four-monitor limit.
- A typical landscape display shows up to four landscape images in a 2x2 grid or three portrait images left-center-right. Other display shapes derive their own layout. Mixed-orientation pages use the dominant orientation, with landscape breaking ties; no packing optimizer is promised.
- Page preparation loads only screen previews. Original pixels are loaded only for hold inspection. Display failure preserves the session, reduces capacity, and visibly repaginates excess images.
- Group order is deterministic advisory-quality order: enabled face result, then sharpness, then stable Main/path order. Quality evidence never selects or deletes automatically.
- Cross-display order follows configured display order, then top-to-bottom and left-to-right. It owns range selection and direct-key assignment.
- Every card shows filename, dimensions, file size, exact-copy count, and enabled advisory quality hints. Every selection and anchor change remains visible on its owning display.
- On the first page, Main-selected members remain selected. If none appear there, the entry anchor is selected exclusively; if it disappeared, the first visible image is selected.
- Comparison ordinary click, Cmd/Ctrl-click, Shift-click, arrows, Home/End, and Cmd/Ctrl+A follow Main-style selection within the current page. Spatial arrows continue onto the next configured display when crossing an edge. Page changes preserve that page's session-only draft selection; no action affects a hidden page.
- The first 36 visible images receive direct keys in order: 0 through 9, then A through Z. Each assigned key is printed on its card and assignments refresh on page changes. A bare assigned key toggles its image like Cmd/Ctrl-click, and auto-repeat is ignored. Modified keystrokes are not direct image keys and continue through ordinary command handling. Direct keys do nothing while a modal, editable field, menu, or another interactive control owns input. Images after 36 remain normally selectable without another key scheme.
- A bare assigned letter, including F, belongs to its Comparison image. Unassigned F and Space have no Comparison action; hold inspection provides detailed examination without nesting another viewer.
- Delete/Backspace Trashes selected images; confirmed Shift+Delete permanently deletes them. Not similar removes selected members from this group without touching files and preserves that user-authored exclusion through automatic rebuilds.
- Enter retains the selected images and Trashes every visible unselected image on the current page. Double-click exclusively selects its image and immediately invokes the same keep-one/Trash-visible-complement action. Shift+Enter permanently deletes the visible complement after confirmation.
- Enter with no selection does nothing and explains that at least one image must be selected. Trash all visible is a separate labeled action. If every visible image is selected, the page completes without a filesystem operation.
- Comparison's default actions follow Confirm Trash; when enabled, confirmation states exact keep and Trash counts. Every permanent action confirms. Cancellation returns to the same page with selection and anchor unchanged.
- Page Up/Page Down and visible Previous/Next controls move among undecided pages without wrapping and show current-page and remaining counts.
- Successful decisions consume only the current page. Retained images do not stay pinned into later pages; browsing may preserve undecided page drafts, and no shortlist or final-round subsystem is added.
- After the last page, Comparison closes, refreshes Main, and anchors after the original group, then the nearest previous survivor, then none. Leaving early restores the surviving Main selection and anchor without applying an uncommitted keep/discard decision.
- Partial file-operation success remains completed. Successful removals leave; failed intended removals remain visible with a persistent explanation and may be retried without rolling back successful work.
- A vanished visible image is removed while surviving selection remains and its vacancy is filled from the undecided queue when possible. A vanished anchor recovers to the next selected, previous selected, next/previous visible image, then none. If fewer than two comparable members remain, Comparison closes without applying an uncommitted decision.
- A failed image preview retains its card, filename, known attributes, selection, file actions, and Open in Default App while explaining the failure.

## Visual and focus boundaries

- Main media tiles, similar-image thumbnails, video snapshot thumbnails, the inner media in drag previews, Other-file selection rows, and destination drop targets use clipped media/row treatments with selection, focus, and drop feedback painted inside their bounds.
- Other-file entries and destination entries are flat full-width rows rather than rounded floating pills. Destination feedback is never hidden under adjacent entries. Full-row hit targets and visible keyboard focus remain intact.
- Item and row surfaces use the ordinary pointer at rest; a drag cursor appears only after drag activation.
- Sidebar filled rows, Comparison decision cards, Preview/Quick View canvases, modal cards, buttons, fields, and the outer drag-payload card retain borders or rounding that communicate a real container or state.
- One centralized shortcut owner prevents notifications, nested controls, and transient surfaces from accidentally invoking another view's command while preserving normal accessible behavior in genuinely focused controls.
