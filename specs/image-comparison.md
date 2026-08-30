# Image Comparison

## Purpose and ownership

Comparison is a temporary decision workspace for deciding which images to retain from one similar-image group. It is not a library section, persistent Preview, or general viewer. Comparison owns its frozen group membership, visible pages, draft page selections, display allocation, and page-local keep or discard actions. Main retains the underlying library selection and anchor until Comparison finishes or closes.

## Entry and session membership

Main Enter or the visible Compare action may open Comparison only when the complete Main selection resolves to one live similar-image group. Comparison opens the complete group, even when Main selected only part of it. If fewer than two comparable live members remain, OneCopy stays in Main and explains that there is nothing to compare.

Membership freezes when the session opens. Images discovered or automatically regrouped later wait for the next session. Missing images leave the active session. A deliberate Not similar decision also removes its selected images immediately.

OneCopy never restores an unfinished Comparison session, its page, or its draft selections after restart.

## Group and page capacity

A similar-image group has no size limit. Comparison divides any group into pages.

`Maximum images in Comparison` limits only the number displayed simultaneously and defaults to 16. Its minimum is 2. The actual page size is the least of the configured maximum, the remaining undecided images, and the legible capacity of the eligible configured displays.

Comparison uses as many eligible displays as needed to reach the current page size and leaves unnecessary displays uncovered. There is no four-display limit. A typical landscape display shows up to four landscape images in a 2x2 grid or three portrait images from left to right. Other display shapes use a layout suited to that display. The dominant orientation of the current page selects the ordinary layout; a tie uses the landscape layout.

Only the current page's viewing content is prepared ahead. Original pixels are obtained on demand for inspection. If a display window becomes unavailable, Comparison preserves the session and draft selections, recomputes capacity from the surviving displays, and moves excess images to later pages.

## Order and presentation

Comparison uses a deterministic suggested-quality order: enabled face-quality result, then sharpness, then stable Main or path order for ties. Quality facts are visibly advisory. They never select an image or create a keep or deletion decision.

Configured display order determines cross-display order. Within a display, order is top to bottom and then left to right. This stable order also governs range selection and direct image keys.

Each card uses the fitted and hold-inspection behavior owned by `content-presentation.md`. The card shows its filename, dimensions, file size, exact-copy count, and enabled advisory quality hints. An outer card border may communicate selection and decision state; the image remains cleanly contained within the card.

A failed preview remains a usable card with its filename, known facts, selection state, file actions, and Open in Default App action. Comparison explains the preview failure rather than removing the image from the decision.

## Draft selection

Selection is neutral until the user invokes an action. It is not an automatic keeper decision.

On the first page, Comparison preserves Main-selected group members that are visible there. If none are visible, it exclusively selects the entry anchor. If the entry anchor is gone, it selects the first visible image. No quality score may create the initial selection.

Ordinary click exclusively selects a card. Clicking the sole selected card leaves it selected. Cmd/Ctrl-click toggles one card and makes a newly selected card the anchor. Shift-click adjusts a continuous range from the anchor in stable cross-display order.

Arrow keys move the anchor spatially through the grids and across display edges. Without Shift they exclusively select the destination; with Shift they extend the range. Home and End select the first or last image on the current page, with Shift extending to that bound. Cmd/Ctrl+A selects the current page only and never selects hidden later pages.

Each undecided page retains its draft selection while the user visits another undecided page. Selection and anchor are visibly indicated on the display containing them.

## Direct image keys

The first 36 visible images receive bare direct keys in stable order: `0-9`, then `A-Z`. Each assigned key is visibly printed on its card and is reassigned when the page changes. Pressing an assigned key toggles that image as Cmd/Ctrl-click would.

Auto-repeat does not repeat a direct-key toggle. Cmd/Ctrl/Alt-modified keys retain their normal application or operating-system meaning, and shifted symbols do not activate image keys. Direct image keys are inactive while a modal, editable field, menu, or interactive control owns keyboard input.

The thirty-seventh and later visible images have no direct key and remain fully selectable through pointer and grid navigation. Comparison has no multi-character key system, modifier alphabet, alternate shortcut mode, or key subpages.

When `F` is assigned to a visible card, bare `F` is that card's direct key and does not enter fullscreen. Other assigned letters similarly keep their visible Comparison meaning.

## Not similar

Not similar removes the selected images from the similarity group without changing their files. The exclusion is a user-authored decision that automatic similarity rebuilding respects. Settings provides a deliberate way to clear these exclusions.

If Not similar leaves fewer than two comparable images in the session, Comparison closes without applying any uncommitted keep or discard decision.

## File actions on the selection

Delete and Backspace request recoverable deletion of the selected images themselves. Shift+Delete requests permanent deletion of the selected images and always follows the permanent-deletion confirmation path. These commands do not reinterpret the selected images as keepers.

Open in Default App acts on a chosen selected logical image through its deterministic representative copy. Reveal in File Manager continues to let the user choose a physical copy.

Space has no Comparison action. Comparison does not open a nested Quick View for an implicit current image; hold inspection provides the image-level examination gesture. Escape or Close leaves Comparison without applying its draft selection as a keep or discard decision.

## Page decision

Enter retains the selected images and requests recoverable deletion of every other image on the current visible page. It never affects an unseen page. With no selection, Enter does nothing and explains that at least one image must be selected to keep. Trashing every visible image is a separate explicit labelled action.

Double-click exclusively selects the clicked image and immediately performs the same page-local decision as Enter: retain it and request recoverable deletion of the other visible images.

Shift+Enter retains the selected images and requests permanent deletion of the visible complement. Every permanent page decision requires confirmation. Recoverable page decisions follow the ordinary Confirm Trash setting; when confirmation is enabled, it states the exact numbers to retain and Trash.

If every visible image is selected, the page completes without a filesystem operation. Cancelling a confirmation returns to the same page with its draft selection and anchor unchanged.

The file-operation contract executes the requested recoverable or permanent deletion. Comparison does not overwrite, move, or publish files itself.

## Page progression

Page Up, Page Down, Previous Page, and Next Page move among undecided pages without wrapping. The interface shows the current page and remaining count. Browsing an undecided page does not affect files.

A successful Enter or double-click decision consumes the current page. Selected images are recorded as retained, successfully Trashed images leave the library, and the next undecided images fill the available displays. Retained images do not remain pinned into every later page and reduce its useful capacity.

Comparison continues until every original member is decided or the user exits. If retained images still form a similar group, the user may compare that smaller group in a later session; Comparison does not create a shortlist or forced final round.

After the final page is decided, Comparison closes and Main refreshes. Main places its anchor after the original group; if no such survivor exists, it uses the nearest previous survivor, then none, and keeps the result visible.

## Changes, failures, and exit

A newly discovered similar image does not enter the active session. It becomes eligible the next time the group opens.

When a visible image disappears, Comparison removes it, preserves the surviving selection, and fills the vacancy from the undecided queue when possible. If the anchor disappears, recovery prefers the next selected image, then the previous selected image, then the next or previous visible image, then none. If fewer than two comparable images remain, Comparison closes without applying uncommitted decisions.

Completed file actions remain completed. After a partial recoverable-deletion result, successful images leave and failed intended deletions remain visible with a persistent explanation. Retry re-evaluates those failed intended deletions against current state, targets no successful or retained image, and obtains a new confirmation whenever current confirmation policy requires one. Cancellation stops at the next safe filesystem boundary and does not roll completed work back.

If OneCopy cannot durably record the promised failure explanation, it stops the affected operation rather than continuing without that record.

Failure of an auxiliary display preserves the session and repaginates on surviving displays. Failure of the Main comparison renderer preserves files and recorded completed work, closes auxiliary presentation safely, and provides a visible reload or recovery path.

Persistent Preview is hidden while Comparison is open. On exit, OneCopy restores its prior pane or window placement and lets it follow Main's recovered anchor. Exit during an active mutation stops admission of new work and follows the normal mutation-quiescence contract.
