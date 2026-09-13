# Viewing Sessions

## Ownership

- Main owns library order, selection, anchor, and resumable work position. Details and persistent Preview follow Main and never create another library selection.
- Persistent Preview is one follower with pane and separate-window placements. Only one placement is active at a time.
- Quick View and true fullscreen are two presentations of one temporary viewer session. They share one frozen sequence, current item, Main relationship, prepared content, deletion state, inspection state, playback state, transcript state, failures, and notifications.
- Comparison is a separate workspace governed by `image-comparison.md`; it is not a Preview or transient-viewer mode.

## Persistent Preview lifecycle

- Opening Preview with a Main anchor shows that item immediately. Opening without an anchor leaves Preview open with a truthful no-selection state and follows the next anchor.
- A multi-selection shows only its anchor and makes the selected count visible. Preview is not a slideshow and does not own independent item navigation.
- Pane versus separate-window placement and open state are remembered across restart. The separate Preview's display, usable ordinary bounds, and normal/maximized mode are also durable workspace placement, owned by one native record rather than frontend interface state or a parallel geometry fallback. If Preview was open at normal exit, it returns only after Main restores its section and anchor. Playback position, text scroll, text selection, and transient inspection do not restore after restart.
- On first separate-window use without a saved placement, choose the first currently available screen in configured priority order other than Main's actual screen. When Main is on the configured primary screen, this chooses the secondary screen. Resolve a straddling Main by greatest display overlap, with configured priority breaking ties. Establish useful normal bounds on the chosen display before maximizing there, never fullscreen. With only one available screen, float Preview visibly above Main at normal size without maximizing.
- Later openings, including after app restart, restore the user's display, usable ordinary bounds, and normal/maximized mode while that display remains available. Restore geometry and maximized mode before showing the window. Preview deliberately differs from Main on macOS: Main starts at normal size, where restoring zoom would remove the expected normal reopening, while Preview starts maximized and must preserve a direct path back to that default presentation. If the saved display is unavailable, rerun first-use allocation; do not relocate Preview merely because Main now shares its display. Fullscreen and minimized geometry never overwrite ordinary bounds or maximized mode. Live display loss remains the operating system's responsibility; reopening resolves against the displays then available.
- Switching placement moves the same Preview session without leaving a second copy open. The same live item retains audio/video playback, transcript state, text scroll, session encoding, wrap state, and expanded attributes; transient image or video inspection ends during the move. Explicitly showing the separate window may bring it forward but does not make it permanently topmost or steal command focus from Main.
- Closing Preview stops following without changing Main selection or anchor. Placement remains remembered for the next open. Closing also ends Preview-owned playback unless the transient viewer already owns that same live session.
- The separate Preview window keeps a lightweight footer showing the current filename, `N selected` when Main has a multi-selection, and a short click-and-hold inspection hint.

## Preview focus and commands

- The pane never becomes a second item-navigation context. Main continues to own selection and command position.
- When the separate Preview window is focused and no native or body-specific interactive control owns input, it forwards Arrow, Page, Home, End, Shift-range, content-specific Enter, Delete/Backspace, and confirmed permanent-deletion commands to Main. Read-only text selection and document scrolling remain local to the body; Main resumes command ownership when that body does not consume the key.
- Space has no separate Preview-window action. `F` toggles true fullscreen on that same live Main-following Preview window; it never starts a frozen Main viewer. `F` or Escape from fullscreen restores the ordinary Preview window and keeps command focus there. Escape from ordinary Preview closes it. Changing Main's anchor continues to update fullscreen Preview.
- Ordinary and double-click have no Preview-level image action. Video picture click and media controls retain the content actions defined by `content-presentation.md`.
- Delete/Backspace in persistent Preview Trashes Main's complete selection for every read-only image, video, audio, text, or attributes body. A multi-item selection always receives exact-count review; confirmed permanent deletion has the same scope. Only a genuinely editable control consumes deletion keys for editing. Holding Enter, Delete, or Backspace never repeats a forwarded entry or file action against a newly opened or recovered context.

## Transient viewer entry and sequence

- From Main, Space opens Quick View and `F` opens true fullscreen. No selection prevents entry with a plain explanation.
- One selected item freezes the whole current section in displayed order and starts at the anchor. Multiple selected items freeze only that selected subset in displayed order and start at the anchor.
- Later discovery, sorting, regrouping, and section changes do not rewrite the active sequence. External disappearance, loss of review eligibility under `library-visibility.md`, and successful current-item deletion remove only the affected item from that frozen sequence.
- Whole-section navigation makes the viewed item Main's exclusive selection and anchor. Selected-subset navigation preserves the selected set and moves only its anchor.
- Main keeps the current anchor visible behind the transient presentation, and persistent Preview follows it without becoming another playback owner.
- The transient session never restores after restart.

## Quick View and true fullscreen

| Current presentation | Space | `F` | Escape |
|---|---|---|---|
| Main | Open Quick View | Open true fullscreen | No app action |
| Quick View | Return to Main | Switch to true fullscreen | Return to Main |
| True fullscreen | Switch to Quick View | Return to Main | Return to Main |

- Presentation switches retain the same session without reloading content, starting another player, reapplying autoplay, or resetting navigation, inspection, transcript, failure, or notification state.
- Quick View is a temporary presentation over Main's usable content area. True fullscreen fills one physical display and hides OneCopy chrome plus the operating-system menu bar or taskbar. On macOS, where Dock visibility is application-global, OneCopy auto-hides the Dock only while a OneCopy full-display presentation occupies the display that currently reserves Dock space; fullscreen on another display leaves the Dock visible on its own display.
- macOS true fullscreen does not use Spaces fullscreen. One reusable borderless viewer presentation covers the invoking display while Main remains in its workspace. Embedded or native media controls do not start a competing fullscreen mode.
- Fullscreen enter, leave, close, error, and shutdown transitions are serialized. A transient Main viewer returns focus to Main; persistent Preview returns focus to its own window. Fullscreen geometry belongs to each window's live session, while process-wide system-chrome policy belongs to the application and is derived from the displays occupied by every live full-display presentation. Switching to another application restores the menu bar and Dock without changing any still-open fullscreen window's geometry, style, or session. Returning to OneCopy reapplies only the system-chrome changes those presentations require. Closing one fullscreen presentation does not dismantle another.
- `F` follows the focused workspace: Main invokes the transient viewer; separate Preview toggles its live follower. A Preview merely visible beside Main does not change Main's command meaning.
- Double-click never enters fullscreen. Fullscreen remains discoverable through `F` and a visible fullscreen control.

## Transient navigation and focus

- While Quick View or fullscreen is active, it owns app commands so the hidden Main grid and sidebar cannot react.
- Viewer entry and presentation return explicitly focus a neutral viewer command surface, never a navigation button chosen by DOM order. Enter on that surface cannot accidentally activate Previous or Next. Topmost dialogs and ongoing text composition take precedence over viewer commands.
- Image and video sequences use Left/Right and Page Up/Page Down for previous and next, Home/End for bounds, and do not wrap. Up/Down has no fitted-media viewer action.
- Mixed Other-file sequences always use Left/Right for previous and next. Audio uses Page Up and Page Down for previous and next plus Home and End for sequence bounds; Up and Down remain available to real controls. Focused text or attributes bodies use Up/Down, Page Up/Page Down, and Home/End for their document rather than sequence navigation.
- Real media controls retain suitable Enter and Arrow accessibility. Space, `F`, and Escape remain transient-viewer commands rather than becoming media-control shortcuts.
- Wheel or trackpad scrolling over a fitted image does not navigate the transient sequence.
- The transient presentation shows the current filename, sequence position, visible previous/next controls, and Close through lightweight chrome that does not permanently reserve a thick frame.
- Closing returns focus to Main with the current anchor visible.

## Transient deletion and disappearance

- Delete/Backspace in Quick View or fullscreen Trashes only the displayed logical item, never a hidden Main multi-selection. Confirmed permanent deletion has the same current-item scope. Holding either deletion key never repeats into the next recovered item.
- Success removes the item from the frozen sequence and Main selection, then chooses the next item, the previous item, or closes when none remain.
- Cancellation or failure preserves the surviving item and sequence when possible. External disappearance uses the same next, previous, then close recovery.
- Playback and other app-owned readers are released before the operation. If the operation fails and the same media survives, `content-presentation.md` governs restoration of its live playback state.

## Failure and notification behavior

- Missing prepared content shows a truthful working state. Presentation failure preserves filename, known attributes, navigation, deletion, and external-open actions instead of making the item disappear.
- Failure to obtain original image pixels preserves an already working fitted image. Unsupported media playback preserves its poster or attributes and ordinary file actions.
- A failed separate Preview window preserves Main state and offers pane placement. A failed fullscreen transition returns safely to Quick View or Main.
- Persistent notifications belong above viewing presentations. Closing or switching Quick View/fullscreen never dismisses an important notification or diverts app-level navigation.
