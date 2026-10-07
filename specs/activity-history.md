# Activity History

## Observation and access

Records is the diagnostic surface for activity, logs, Issues and notifications. It shows individual recorded events with filters and details. Reading history never admits, retries, pauses or replays work. Issues remains the immediately actionable user-facing list of current problems; Background Work owns work controls.

Meaningful work is recorded independently of debug logging. Internal selection, viewport, routing and similar diagnostic transitions remain developer-only detail. Runtime logs retain their independent structured diagnostic role.

## Retained evidence

Events retain their operation and session identifiers, available progress and outcomes. Missing start or terminal evidence remains unknown; inactivity or an application restart never implies success. Instants are stored in UTC and elapsed measurements use same-session monotonic evidence.

History survives ordinary restart and upgrades. Removing a presentation surface does not erase its recorded events or change retention. Failure to record activity is logged without changing independent work, and unavailable history has an explicit error rather than looking empty.
