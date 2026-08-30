# Failure Reporting and Recovery

## Containment

- Every ordinary failure OneCopy can receive from filesystems, databases, managed tools, background workers, watchers, application commands, event delivery, or interface code is contained at an owning boundary.
- A contained failure stops the smallest scope that can no longer proceed safely. Continuing silently is never an error-handling policy.
- Failures requiring user attention become restart-persistent Issues with the attempted action, useful affected paths, ordinary-language explanation, and first/latest occurrence times. Technical details also go to the application log.
- An Issue recurs as the same condition rather than producing unlimited duplicate entries. Successful recheck or retry may clear recoverable conditions; operation failures remain until dismissed or otherwise resolved.
- If OneCopy cannot save an Issue, it stops the affected operation and presents the failure directly instead of continuing without a durable record.
- Every long-lived background worker has an outer failure boundary. Unexpected worker failure publishes a terminal failed state, releases owned resources, records the failure when possible, and leaves an explicit retry or repair path.
- Production code does not deliberately terminate through unchecked assumptions where an ordinary error can be returned or contained.
- A drawing failure produces a visible reload path. Escaped event-handler and asynchronous failures are logged and surfaced rather than merely disappearing.
- Errors during recovery are themselves contained and reported; recovery does not recursively retry without limit.
- The contract does not promise survival from power loss, operating-system termination, exhausted process memory, or a fatal failure inside the operating system or a native dependency. Durable completed steps and startup reconciliation limit the consequences of such termination.

## Notifications and Issues

- A timed notification is minor and safely missable. Its display duration is configurable and defaults to six seconds, and its timer pauses while hovered or focused.
- A persistent notification remains until dismissed but does not steal focus, trap interaction, or become a modal. A modal is reserved for a required decision or a state in which OneCopy cannot safely continue.
- Notification ownership sits above Preview, Quick View, and fullscreen. Closing or switching a transient view never loses or implicitly dismisses a persistent notification, and notification focus never steals those views' app-level shortcuts.
- Every warning or error notification enters restart-persistent history immediately. Issues separates current actionable conditions from recent notification history; dismissing a notification does not erase its history entry.
- Repeated equivalent background failures are coalesced with occurrence counts and times. Batch work reports one useful summary rather than flooding the interface with one notification per file.
- Initial recent-history retention is the newest 500 entries or 30 days, whichever boundary is reached first. These tuning values remain subject to the later Settings-policy audit rather than silently becoming immutable architecture.
- Rebuild library index is the explicit lifetime exception: it clears active Issues and recent notification history with the reconstructible index. Ordinary restart and notification dismissal do not.
- Fatal drawing or renderer failure retains a visible reload or recovery surface instead of being reduced to a disappearing notification.
