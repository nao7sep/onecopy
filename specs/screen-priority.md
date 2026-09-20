# Screen Priority

## The order

- When two or more displays are connected, OneCopy keeps a user-arranged priority order over them, reachable in Settings under Appearance. One display makes the order meaningless, and OneCopy offers nothing to arrange.
- The order names displays, not slots. A display is identified by its reported name together with where it sits, because a matched pair reports one name and one resolution and would otherwise be a single entry. Rearranging the displays in the operating system's own layout therefore produces new entries.
- The order covers every connected display. One the saved order does not name — newly attached, or one whose position changed — follows every named display, in the order the system reports them.
- The order is machine-local workspace state rather than a configured value, since a display identity means nothing on another computer. It is not part of the Settings draft: a move takes effect at once and needs no Save, and discarding unsaved settings does not undo it.

## Arranging it

- A display moves one place at a time, through controls that need no pointer drag. A display already first cannot move up and one already last cannot move down; those controls stay in place and inert rather than disappearing, so the list keeps its shape as a display travels through it.
- Identification labels every connected display at once with its current rank, so the order can be matched to the desk in front of the user. Only one identification runs at a time, and the labels dismiss themselves.
- Each row says where its display sits before it says what the display is called, because position is the only fact that distinguishes one member of a matched pair from the other.

## Where the order is used

- The order chooses the separate Preview window's display on first use, per `viewing-sessions.md`, and sets Comparison's cross-display order, per `image-comparison.md`.
- Both exclude Main's current display when they open an auxiliary window, so the first entry in the order is not necessarily the display they take.
- The order is a preference over the displays that happen to be connected, never a claim that a display exists. An entry whose display is absent is skipped rather than held open.

## Boundaries

- Preview's remembered display, ordinary bounds, and maximized mode are durable window placement owned by `viewing-sessions.md`. This order only chooses a display when there is nothing remembered to restore.
- Comparison's page size, per-display capacity, and grids come from display shape and the configured maximum, per `image-comparison.md`.
- A failure to read the connected displays, to identify them, or to save a move is contained and presented under `failures-and-recovery.md`, and leaves the last saved order in force.
