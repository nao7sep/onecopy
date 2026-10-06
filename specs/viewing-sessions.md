# Viewing Sessions

## Ownership

- Main owns library order, selection, anchor, and resumable work position. Details and persistent Preview follow Main and never create another library selection.
- Persistent Preview is one follower with pane and separate-window placements. Only one placement is active at a time.
- Comparison is a separate workspace governed by `image-comparison.md`; it is not a Preview or fullscreen-view mode.

## Persistent Preview lifecycle

- A multi-selection shows only its anchor and makes the selected count visible. Preview is not a slideshow and does not own independent item navigation.
- Switching placement moves the same Preview session without leaving a second copy open. The same live item retains audio/video playback, transcript state, text scroll, session encoding, wrap state, and expanded attributes; transient image or video inspection ends during the move.
- Closing Preview stops following without changing Main selection or anchor. Placement remains remembered for the next open. Closing also ends Preview-owned playback unless the fullscreen view already owns that same live session.

## Preview focus and commands

- The pane never becomes a second item-navigation context. Main continues to own selection and command position.
- Ordinary and double-click have no Preview-level image action. Video picture click and media controls retain the content actions defined by `content-presentation.md`.

## Fullscreen view

- The fullscreen view never restores after restart.
- Wheel or trackpad scrolling over a fitted image does not navigate the sequence.
- The fullscreen view shows the current filename, sequence position, visible previous/next controls, and Close through lightweight chrome that does not permanently reserve a thick frame.

## Fullscreen view deletion and disappearance

- Cancellation or failure preserves the surviving item and sequence when possible.
- Playback and other app-owned readers are released before the operation. If the operation fails and the same media survives, `content-presentation.md` governs restoration of its live playback state.

## Failure behavior

- Missing prepared content shows a truthful working state. Presentation failure preserves filename, known attributes, navigation, deletion, and external-open actions instead of making the item disappear.
- Failure to obtain original image pixels preserves an already working fitted image. Unsupported media playback preserves its poster or attributes and ordinary file actions.
