# Library Items

This contract defines how OneCopy turns physical main files into logical library items and how dates, representatives, companions, and date-based section state belong to those items. Discovery and background maintenance are owned by `library-maintenance.md`; file-operation effects are owned by `file-operations.md`.

## Content identity and physical copies

Complete content bytes are the sole identity of a logical item. Byte-identical main files form one logical item regardless of their directories, filenames, filename case, or timestamps. A file whose content has not yet been read may remain known only as an individual physical file until identity work completes.

Each physical main copy retains its own path, current filename, availability, and date evidence. Companion records remain relationships of physical main copies; they do not become main copies or influence the main-copy count merely because their bytes match another file.

Names and timestamps never determine whether two main files are one logical item. Database insertion order and a majority of matching filenames have no product meaning.

## Date evidence and completion

Each physical copy contributes its own date evidence because byte-identical copies may carry different filesystem or embedded dates. A date is usable only when it is acceptable under the configured date-selection policy.

Date work has distinct incomplete and completed states:

- A logical item remains pending while any currently known available copy still has unchecked date evidence.
- When every currently known available copy has completed date checking and none supplies an acceptable date, the logical item is completed as Undated.
- Completed Undated items are not reopened repeatedly merely because they remain Undated.

Changing the date-selection policy recomputes logical-item dates from saved evidence without reopening unchanged files. New or apparently changed copies may supply new evidence when maintenance processes them, including evidence that moves a completed Undated item into a dated section.

## Representative copy

Every logical item has one deterministic representative among its currently available physical copies. Copies are ranked by the following rules, in order:

1. A copy with an acceptable date precedes a copy without one.
2. An earlier acceptable date precedes a later acceptable date.
3. Equal dates are ordered by complete path without regard to case.
4. If those paths still tie, their exact complete paths break the tie.

The first copy in that order is the representative. A completed Undated item uses the same path ordering because all of its copies tie on date.

The representative supplies the logical item's chosen date when it has one and supplies its current displayed and output filename. The rule applies to every filename difference, not only capitalization. Other copies do not outvote the representative's filename.

When OneCopy detects that the representative disappeared or became unavailable, it applies the same ordering to the remaining available copies. The replacement representative supplies both the resulting date and its current filename.

## Companion relationships

An ordinary companion pairs with a physical main copy only when both files are in the same directory and their filename stems match without regard to case. Pairing never searches another directory or drive.

Live Photos are the sole approved exception to stem matching. Two files in the same directory may form a Live Photo relationship when their embedded Apple content identifiers match exactly even if their filenames differ. Live Photo pairing never crosses directories.

Each physical main copy owns its local companion relationships. When byte-identical main copies in different directories have companions, the logical item retains every one of those local relationships.

Companions attached to different main copies do not need identical content. Divergent companion bytes do not split the main logical item, invalidate the relationship, or turn companion identity into a requirement for library grouping.

## Section state

Date-based section state follows the logical item's current completed evidence. Pending date work remains distinguishable from completed Undated state. A detected source change, disappearance, newly completed evidence, or date-policy change may reclassify the logical item without changing its content identity.

Image and video identities occupy their corresponding media sections. Audio, text, documents, archives, executables, and every other non-image, non-video identity occupy Other files. Presentation capability never changes that classification: an audio presentation remains an Other-file presentation, and falling back from a specialized body to text or attributes never moves an item to another section.

Section state and representative choice always derive from current known evidence. They do not trigger periodic content rescans of otherwise unchanged physical copies.
