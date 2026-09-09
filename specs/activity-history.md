# Activity History

## Observation and access

Activity Trace is available during ordinary use so users can see what OneCopy is doing, including work that takes time. It observes existing operation owners; recording, reading, expanding, and navigating history never admits, retries, pauses, or replays work. Issues and failure recovery remain governed by `failures-and-recovery.md`.

Meaningful work is recorded independently of debug logging. Internal selection, viewport, routing, and similar diagnostic transitions remain developer-only detail rather than flooding ordinary history. Runtime logs retain their independent structured diagnostic role; Activity has no Copy JSONL control.

## Operation lifetime and retained history

One operation has one stable row from its first recorded admission or start through its terminal outcome. Available progress appears while work is open; completion updates the same row rather than inserting each lifecycle event as another task. An operation's identity and initial position do not change when later events arrive.

Operation summaries derive from the retained lifecycle, not whichever raw events happen to fit in a loaded page. Missing start or terminal evidence remains explicitly unknown; no elapsed-time heuristic, quiet queue, or application restart implies success. An unfinished operation from an earlier app run is not presented as currently running.

History survives ordinary restart and upgrades. Raw diagnostics and operation summaries share one history authority; a projection used for bounded reads is reconstructible from retained events and never becomes a job ledger. Failure to record activity is logged without changing independent work, and unavailable history has an explicit error rather than looking empty.

## Reading and presentation

History uses bounded operation pages, complete incremental catch-up, and bounded technical-event pages. Bursts between refreshes do not silently lose work. Older-page and live-update requests belong to the current open surface; late results from a closed or superseded surface cannot change its replacement.

New operations appear above older ones. While reading below the top, the visible operation and its pixel position remain stable through insertions, progress, expansion, and completion. New work remains reachable by scrolling up; updates never force the reader back to the top.

Collapsed rows communicate local time, action, available target or scope, progress or duration, and truthful outcome in ordinary language. Expanded rows organize technical lifecycle information and show shared operation/session identifiers once rather than repeating them in every event. Instants are stored in UTC; durations use same-session monotonic evidence rather than local wall-clock subtraction.

File targets use stable indexed identity and Main's diagnostic-navigation contract in `main-review.md`. Missing or hidden-only targets remain unavailable rather than revealing another file or bypassing visibility. Target references do not add private paths, media content, or transcripts to the diagnostic export stream.
