# Failure Reporting and Recovery

- Every ordinary failure OneCopy can receive from filesystems, databases, managed tools, background workers, watchers, application commands, event delivery, or interface code is contained at an owning boundary.
- A contained failure stops the smallest scope that can no longer proceed safely. Continuing silently is never an error-handling policy.
- Failures requiring user attention become restart-persistent Issues with useful affected paths and explanations. Technical details also go to the application log.
- An Issue recurs as the same condition rather than producing unlimited duplicate entries. Successful recheck or retry may clear recoverable conditions; operation failures remain until dismissed or otherwise resolved.
- If OneCopy cannot save an Issue, it stops the affected operation and presents the failure directly instead of continuing without a durable record.
- Every long-lived background worker has an outer failure boundary. Unexpected worker failure publishes a terminal failed state, releases owned resources, records the failure when possible, and leaves an explicit retry or repair path.
- Production code does not deliberately terminate through unchecked assumptions where an ordinary error can be returned or contained.
- A drawing failure produces a visible reload path. Escaped event-handler and asynchronous failures are logged and surfaced rather than merely disappearing.
- Errors during recovery are themselves contained and reported; recovery does not recursively retry without limit.
- The contract does not promise survival from power loss, operating-system termination, exhausted process memory, or a fatal failure inside the operating system or a native dependency. Durable completed steps and startup reconciliation limit the consequences of such termination.
