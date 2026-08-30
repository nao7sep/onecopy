# Main Review

## Scope

Main is the library workspace for choosing a section, maintaining a visible selection, resuming a work position, inspecting the anchor through Details and Preview, and invoking review or file-operation actions. Main owns the displayed order, selection set, range origin, and one anchor. Viewer sessions, Comparison decisions, background discovery, and filesystem effects have separate contracts.

## Selection and anchor

The selection is the set a Main operation targets. The anchor is the last deliberately selected or navigated item, the subject followed by Details and persistent Preview, the starting item supplied to a viewer, and the resumable work position.

Ordinary click exclusively selects an item. Clicking the sole selected item leaves it selected. Cmd/Ctrl-click toggles one item without disturbing the others. Shift-click and Shift-modified navigation form or adjust a continuous displayed range from a stable range origin. Navigation without Shift exclusively selects its destination. Clicking empty item-area space clears both selection and anchor.

Dragging an unselected item exclusively selects and drags it. Crossing the drag threshold from a selected member drags the complete selection. Releasing before a drag begins remains an ordinary click and collapses the selection normally.

The anchor is always a selected item when a selection exists. Details and persistent Preview show the anchor rather than choosing another member of a multi-selection. A deliberate selection or navigation change updates the anchor and keeps it visible.

## Section entry and restoration

The first visit to a nonempty section selects its first displayed item. Returning to a section during the same app run restores that section's remembered anchor as an exclusive selection; an earlier multi-selection does not remain active while the user works elsewhere.

Across app restarts, OneCopy restores the last-open section and its anchor. It does not restore an independently active selection for every section.

If a remembered or current anchor is gone, OneCopy uses its former displayed position: the next surviving item, then the previous surviving item, then no selection. When part of a multi-selection survives, the selection remains and anchor recovery first prefers the next selected survivor, then the previous selected survivor, before choosing an unselected neighbor. The persisted work position must be sufficient to recover near a missing anchor rather than falling back to the beginning of a large section.

Every restored or recovered anchor is scrolled into view. Restoration and disappearance recovery show useful surrounding context; ordinary navigation scrolls only as far as necessary.

Sorting, refreshed information, and newly discovered items preserve surviving selection and anchor by logical identity. Reordering and insertion never steal selection.

## Focus ownership

Opening a section from the sidebar leaves keyboard focus in the sidebar so section navigation can continue. Right Arrow or Tab enters the item area. Item commands, including Space, Enter, Delete, and Backspace, do not act while the sidebar owns focus.

The Preview pane does not become a competing library-navigation owner. Persistent Preview follows Main's anchor and never creates another selection. Preview-window focus and command forwarding are owned by `viewing-sessions.md`.

Keyboard focus is visibly distinct from hover and selection. Focus and selection treatment stay inside each item or row so neighboring content cannot clip them.

## Shared Main commands

Space requests Quick View for the current anchor and selection. `F` requests the same transient viewing session in true fullscreen. With no selection, OneCopy stays in Main and explains that an item must be selected.

Double-click exclusively selects the clicked item and then requests Quick View. The resulting sequence follows the ordinary one-selection entry rule in `viewing-sessions.md`; double-click does not open Comparison, fullscreen, or an external application.

Delete and Backspace request recoverable deletion of the complete Main selection. Shift+Delete requests permanent deletion of the complete Main selection through its confirmation path.

The resting pointer over an item is the normal clickable pointer. A drag pointer appears only after dragging begins. Click-and-hold inspection is not available on Main thumbnails or rows because the gesture belongs to dragging there.

## Images

Image-tile rendering and preparation failures follow `content-presentation.md`. Main owns the tile's selection, focus, drag, and command state.

Enter and the visible Compare action request Comparison only when every selected item belongs to the same live similar-image group. Comparison receives the complete group, not merely the selected subset. If the selection contains another group, an ungrouped image, or another content type, OneCopy opens nothing and explains that Comparison requires images from one similar group. Comparison itself owns every later keep, discard, paging, and exit decision.

## Videos

Video-tile rendering and preparation failures follow `content-presentation.md`. Main owns the tile's selection, focus, drag, and command state, and a missing poster never makes a known video unselectable or unavailable for file operations.

If persistent Preview is open, it follows a newly anchored video. Main navigation does not raise, focus, open, close, or independently navigate Preview. Rapid navigation may prevent crossed videos from starting before they become the settled anchor.

Enter toggles play or pause only when persistent Preview currently shows a playable anchor video. Enter does not open a player or viewer and does nothing when no player is visible.

If a playing anchor disappears, OneCopy releases it, recovers the Main anchor by the common rule, and lets persistent Preview follow the result.

## Other files

Audio, text, source files, documents, archives, executables, and unknown formats share the Other-files section. Available preview presentation never changes an item's section identity.

Other files use flat, full-width rows containing the filename, useful known attributes, and truthful preparation or error state. Unavailable facts remain unknown. The row's selection, hover, drop, and keyboard-focus treatment stays inside its bounds rather than appearing as a floating rounded pill.

Up and Down navigate rows. Page Up and Page Down move by approximately one visible page. Home and End reach the displayed bounds. Navigation clamps and does not wrap. Left and Right have no row-selection action.

Persistent Preview follows the anchor and chooses the available audio, text, specialized, or attributes presentation. Enter toggles play or pause only when persistent Preview currently shows a playable anchor audio file. Enter has no Main action for text or attributes.

## Details

Details follows Main's anchor and changes together with its logical identity. A multi-selection does not cause Details to choose an arbitrary member. If the anchor is cleared, Details shows no item. If it disappears, Details follows Main's recovered anchor.

Facts, similar-item thumbnails, scene thumbnails, transcript status, and actions shown in Details must belong to the same current anchor. Slow or stale preparation must not combine content from one item with the identity of another.

## Destination rows

Destination entries are flat, full-width list rows. Selected destinations use a contained full-row fill. Drag hover adds a stronger full-row fill and an inset outline that remains above neighboring rows and cannot be clipped by them. Ordinary child rows form a continuous list; deliberate spacing may separate root groups.

Destination admission and file-operation effects are governed by the file-operation contract.
