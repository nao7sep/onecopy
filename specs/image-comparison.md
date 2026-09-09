# Image Comparison

## Purpose and ownership

Comparison is a temporary decision workspace for deciding which images to retain from one similar-image group. It is not a library section, persistent Preview, or general viewer. Comparison owns its frozen group membership, visible pages, active inspection card, explicit page-local keep marks, display allocation, and page-local keep or discard actions. Main retains the underlying library selection and anchor until Comparison finishes or closes.

## Entry and session membership

Main Enter or the visible Compare action may open Comparison only when the complete Main selection resolves to one live similar-image group. Comparison opens the complete group, even when Main selected only part of it. If fewer than two comparable live members remain, OneCopy stays in Main and explains that there is nothing to compare.

Membership freezes when the session opens. Images discovered or automatically regrouped later wait for the next session. Missing images and those losing review eligibility under `library-visibility.md` leave the active session.

OneCopy never restores an unfinished Comparison session, its page, active card, or keep marks after restart.

## Group and page capacity

A similar-image group has no size limit. Comparison divides any group into pages.

`Maximum images in Comparison` limits only the number displayed simultaneously and defaults to 16. Its minimum is 2. The actual page size is the least of the configured maximum, the remaining undecided images, and the legible capacity of the eligible configured displays.

Comparison uses as many eligible displays as needed to reach the current page size and leaves unnecessary displays uncovered. There is no four-display limit. A typical landscape display shows up to four landscape images in a 2x2 grid or three portrait images from left to right. Other display shapes use a layout suited to that display. The dominant orientation of the current page selects the ordinary layout; a tie uses the landscape layout.

Only the current page's viewing content is prepared ahead. Original pixels are obtained on demand for inspection. If a display window becomes unavailable, Comparison preserves the session and keep marks, recomputes capacity from the surviving displays, and moves excess images to later pages.

## Order and presentation

Comparison uses a deterministic suggested-quality order: enabled face-quality result, then sharpness, then stable Main or path order for ties. Quality facts are visibly advisory. They never select an image or create a keep or deletion decision.

Configured display order determines cross-display order. Within a display, order is left to right and then top to bottom. This stable order also governs range selection and direct image keys.

Each card uses the fitted and hold-inspection behavior owned by `content-presentation.md`. The card shows its filename, dimensions, file size, exact-copy count, and enabled advisory quality hints. An outer card border may communicate selection and decision state; the image remains cleanly contained within the card.

A failed preview remains a usable card with its filename, known facts, active and keep-mark state, file actions, and Open in Default App action. Comparison explains the preview failure rather than removing the image from the decision.

## Active card and keep marks

The active card is the current navigation and inspection position. Keep marks are the explicit file-decision draft. They are separate states: activating or navigating to an image never marks it for retention.

Comparison opens on the entry anchor's page without activating any card or creating keep marks. It never imports Main selection as inspection or keep intent. An unvisited page likewise starts without an active card; returning to a visited page restores its explicitly chosen card. No quality score may activate or mark an image.

Ordinary click activates a card without changing its keep mark. Each card has a visible Keep control that toggles only that card's mark and makes it active. Cmd/Ctrl-click may perform the same explicit toggle. Shift-click adjusts a continuous marked range from a stable origin in cross-display order on top of the keep marks that existed when that range began, so deliberate marks outside the range survive as it grows, shrinks, or reverses.

Arrow keys move the active card spatially through the grids and across display edges; with no active card, the first Arrow activates the first visible image. Without Shift they do not change keep marks; with Shift they extend the marked range. Home and End activate the first or last image on the current page, with Shift extending marks to that bound. Space opens the active card in a separate image window and never toggles its keep mark; with no active card it does nothing. Cmd/Ctrl+A marks the current page only and never marks hidden later pages.

Each undecided page retains its keep marks while the user visits another undecided page. Active and marked states are visually distinct on the display containing them.

## Direct image keys

The first 36 visible images receive bare direct keys in stable order: `0-9`, then `A-Z`. Each assigned key is visibly printed on its card and is reassigned when the page changes. Pressing an assigned key explicitly toggles that image's keep mark and makes it active.

Auto-repeat does not repeat a direct-key toggle or Space inspection transition. Cmd/Ctrl/Alt-modified keys retain their normal application or operating-system meaning, and shifted symbols do not activate image keys. Direct image keys are inactive while a modal, editable field, menu, or interactive control owns keyboard input.

The thirty-seventh and later visible images have no direct key and remain fully selectable through pointer and grid navigation. Comparison has no multi-character key system, modifier alphabet, alternate shortcut mode, or key subpages.

When `F` is assigned to a visible card, bare `F` is that card's direct key and does not enter fullscreen. When `F` is unassigned, it does nothing in Comparison. Other assigned letters similarly keep their visible Comparison meaning.

## File actions on the selection

Delete and Backspace request recoverable deletion of the keep-marked images themselves. Deleting more than one image always receives an exact-count review; the single-item direct-Trash preference applies only to one marked image. Shift+Delete requests permanent deletion of the keep-marked images and always follows the permanent-deletion confirmation path. These commands do not reinterpret the marked images as keepers.

Open in Default App acts on the active logical image through its deterministic representative copy. Reveal in File Manager continues to let the user choose a physical copy.

Space opens a separate image window containing the complete active image at the larger available size. Hold inspection remains the distinct original-pixel gesture. Space, Escape, or Close in the image window returns command focus to the invoking Comparison display without changing its page, active card, or keep marks. This image window neither starts Main's frozen viewer sequence nor creates another library selection. Closing Comparison or removing the inspected image closes that window too. Escape or Close in Comparison itself leaves without applying its keep marks as a file decision.

Comparison and its image window own their commands while active. Hidden Main controls never respond to those keys, even if stale DOM focus remains behind. Entry focuses the Comparison item area without activating a card; exit restores Main's item-area focus after its anchor and visibility have recovered. Real controls and topmost dialogs retain their own input, and composition keystrokes never invoke workspace commands.

## Page decision

Enter with no keep marks closes Comparison without mutation, reversing the Main Enter that opened it. With marks, Enter retains the marked images and opens an exact-count review before requesting recoverable deletion of every other image on the current visible page. It never affects an unseen page and never mutates immediately. Trashing every visible image is a separate explicit labelled action and always opens an exact-count review.

Double-click only activates the clicked image for inspection. It never changes keep marks or begins a file action.

Shift+Enter with marks retains the marked images and requests permanent deletion of the visible complement. Every permanent page decision requires exact-count confirmation. The recoverable complement decision is an indirect bulk consequence and therefore always requires the same exact-scope review regardless of the single-item direct-Trash preference.

If every visible image is marked, the page completes without a filesystem operation. Cancelling a confirmation returns to the same page with its keep marks and active card unchanged.

Holding Enter, Delete, or Backspace never repeats a Comparison file decision across recovered images or pages. Navigation keys may repeat.

The file-operation contract executes the requested recoverable or permanent deletion. Comparison does not overwrite, move, or publish files itself.

## Page progression

Page Up, Page Down, Previous Page, and Next Page move among undecided pages without wrapping. The interface shows the current page and remaining count. Browsing an undecided page does not affect files.

A successful Enter decision consumes the current page. Marked images are recorded as retained, successfully Trashed images leave the library, and the next undecided images fill the available displays. Retained images do not remain pinned into every later page and reduce its useful capacity.

Comparison continues until every original member is decided or the user exits. If retained images still form a similar group, the user may compare that smaller group in a later session; Comparison does not create a shortlist or forced final round.

After the final page is decided, Comparison closes and Main refreshes. Main places its anchor after the original group; if no such survivor exists, it uses the nearest previous survivor, then none, and keeps the result visible.

## Changes, failures, and exit

A newly discovered similar image does not enter the active session. It becomes eligible the next time the group opens.

When a visible image disappears, Comparison removes it, preserves the surviving keep marks, and fills the vacancy from the undecided queue when possible. If the active image disappears, recovery prefers the next marked image, then the previous marked image, then the next or previous visible image, then none. If fewer than two comparable images remain, Comparison closes without applying uncommitted decisions.

Completed file actions remain completed. After a partial recoverable-deletion result, successful images leave and failed intended deletions remain visible with a persistent explanation. Retry re-evaluates those failed intended deletions against current state, targets no successful or retained image, and obtains a new confirmation whenever current confirmation policy requires one. Cancellation stops at the next safe filesystem boundary and does not roll completed work back.

If OneCopy cannot durably record the promised failure explanation, it stops the affected operation rather than continuing without that record.

Failure of an auxiliary display preserves the session and repaginates on surviving displays. Failure of the Main comparison renderer preserves files and recorded completed work, closes auxiliary presentation safely, and provides a visible reload or recovery path.

Persistent Preview is hidden while Comparison is open. On exit, OneCopy restores its prior pane or window placement and lets it follow Main's recovered anchor. Exit during an active mutation stops admission of new work and follows the normal mutation-quiescence contract.
