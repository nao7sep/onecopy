# Failures and Recovery

## Contract boundary

This contract owns how OneCopy contains, records, presents, and recovers from ordinary failures. Feature contracts decide what an action means, what state can be retried, and what successful recovery produces. This contract ensures that a failure cannot silently disappear or take down unrelated work when OneCopy can respond safely.

## Containment

Every ordinary failure OneCopy can receive from filesystems, databases, destinations, permissions, managed tools, background workers, watchers, application commands, event delivery, previews, and interface code is contained at an owning boundary.

A contained failure stops the smallest scope that can no longer proceed safely. A file-local failure does not stop independent files. A failure in a shared destination, database, or other common requirement stops all remaining work that depends on it. Continuing silently is never an error-handling policy.

Power loss, operating-system termination, exhausted process memory, and fatal operating-system or native-dependency failures are outside this containment promise. Durable completed steps and startup reconciliation limit their consequences, but OneCopy does not claim that the process can recover while it is unable to run.

## Logs and user-facing records

Technical context belongs in the application log. User-facing records use plain language and identify the attempted action, relevant source or destination when applicable, and a useful reason. Operation conflicts, full storage, unavailable sources, failed cleanup, worker failure, and whole-operation failure must not exist only as temporary interface text.

A failure requiring user attention becomes a restart-persistent Issue. If OneCopy cannot save the promised Issue, it stops the affected work and presents the recording failure directly instead of continuing without a durable explanation.

Repeated occurrences of the same unresolved condition update one record with a count plus useful first and latest occurrence times rather than producing an unlimited stream of duplicates. A successful retry or recheck may resolve a recoverable condition. A record otherwise remains until the user dismisses it or its owning feature establishes that it no longer applies.

## Notifications and modals

OneCopy uses three distinct interruption levels:

- A timed notification reports minor information that is safe to miss. Its display duration is configurable and defaults to six seconds. Hovering or focusing it pauses the timer.
- A persistent notification remains until dismissed but does not steal focus or block unrelated work.
- A modal is reserved for a required decision or a condition under which OneCopy cannot continue safely without acknowledgement.

Notifications belong to the main application frame rather than a transient viewer. Closing Quick View or switching between Quick View and fullscreen does not dismiss or lose a persistent notification. Notifications do not intercept the viewer's navigation or exit commands.

An expected unsupported format or unavailable richer preview remains truthful inside the affected content surface and does not become an Active Issue merely because OneCopy has no suitable decoder. A failed user-requested action is recorded in Recent notification history. A condition that remains unresolved and requires permission, repair, retry, a tool, or another user action appears in Active. One failed action may therefore enter Recent while its unresolved cause remains in Active.

Every warning or error notification is recorded immediately in Recent. A broad operation such as a source check may present one summary notification while retaining the individual affected paths and explanations in its Active details when the condition still needs attention.

## Issues

The Issues surface has two distinct views:

- `Active` contains unresolved conditions that need attention, retry, repair, permission, a required tool, or dismissal.
- `Recent` contains failed requested actions and the history of timed and persistent notifications across ordinary restarts.

Repeated background failures are combined with a count. Issue presentation must remain useful when many files fail; it summarizes the condition without hiding access to the affected files and technical context.

Issues and Recent history are reconstructible library state rather than a permanent operation ledger. They follow the rebuild lifetime defined by `library-maintenance.md`.

## Background-worker failure

Every long-lived background worker has an outer failure boundary. An unexpected worker failure publishes a terminal failed state, releases resources it owns, records the failure when possible, and leaves an explicit Retry, Resume, or repair path. A worker must never stop while its visible state still claims that work is running or healthy.

A failure limited to one input is recorded and processing continues when later inputs remain safe and independent. Failure of shared worker state stops that worker rather than allowing it to continue with unreliable ownership or progress.

## Interface and asynchronous recovery

A drawing or rendering failure produces a visible reload or restart path instead of a blank or frozen surface. Escaped event-handler and asynchronous failures are logged and surfaced through the same notification and Issue system rather than disappearing into a console or abandoned task.

Recovery failures are themselves contained and reported. Recovery does not retry recursively or without limit. When a fallback cannot restore a safe usable state, OneCopy stops the affected surface or operation and leaves the user a direct reload, restart, retry, or repair action.

## Restart behavior

Active Issues survive ordinary application restart until their normal resolution or dismissal boundary. Recent notification history also survives ordinary restart, subject to its retention policy. Restart does not replay a failed or partial destructive operation. Feature owners re-evaluate current state before offering a retry, and completed durable steps remain completed.
