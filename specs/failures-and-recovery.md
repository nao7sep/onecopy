# Failures and Recovery

## Contract boundary

This contract owns how OneCopy contains, records, presents, and recovers from ordinary failures. Feature contracts decide what an action means, what state can be retried, and what successful recovery produces. This contract ensures that a failure cannot silently disappear or take down unrelated work when OneCopy can respond safely.

## Containment

Every ordinary failure OneCopy can receive from filesystems, databases, destinations, permissions, managed tools, background workers, watchers, application commands, event delivery, previews, and interface code is contained at an owning boundary.

A contained failure stops the smallest scope that can no longer proceed safely. A file-local failure does not stop independent files. A failure in a shared destination, database, or other common requirement stops all remaining work that depends on it. Continuing silently is never an error-handling policy.

Power loss, operating-system termination, exhausted process memory, and fatal operating-system or native-dependency failures are outside this containment promise. Durable completed steps and startup reconciliation limit their consequences, but OneCopy does not claim that the process can recover while it is unable to run.

## Logs and user-facing records

Technical context belongs in the application log. User-facing records use plain language and identify the attempted action, relevant source or destination when applicable, and a useful reason. Operation conflicts, full storage, unavailable sources, failed cleanup, worker failure, and whole-operation failure must not exist only as temporary interface text.

A failure requiring user attention becomes an Issue in the current app run, with a retained diagnostic record. If OneCopy cannot save the promised Issue, it stops the affected work and presents the recording failure directly instead of continuing without a durable explanation.

Repeated occurrences of the same visible unresolved condition update one record with a count plus useful first and latest occurrence times rather than producing an unlimited stream of duplicates. A successful retry or recheck may resolve a recoverable condition. Dismissal and resolution remove a record from the live inbox, not from retained diagnostics: the record keeps its original context and the time and reason it left the inbox. Dismiss all applies to every live entry, including entries beyond the loaded page, without changing already archived records or notification history. A later genuinely failed attempt creates a new visible record rather than reviving or modifying the dismissed or resolved one. Reading or refreshing the inbox never constitutes a new attempt, and archived records do not participate in current recovery controls or work eligibility.

## Notifications and modals

Expected command ineligibility is informational feedback, not an application failure. Main's command feedback belongs to the selection or section that the command evaluated; changing that context or successfully replacing the same command clears obsolete feedback. Delayed responses cannot restore it into a newer context. Independent unresolved failures and file-operation results remain with their notification, Issue, or operation owner rather than competing for one untyped status message.

OneCopy uses three distinct interruption levels:

- A timed notification reports minor information that is safe to miss. Its display duration is configurable and defaults to six seconds. Hovering or focusing it pauses the timer.
- A persistent notification remains until dismissed but does not steal focus or block unrelated work.
- A modal is reserved for a required decision or a condition under which OneCopy cannot continue safely without acknowledgement.

Notifications belong to the main application frame rather than a transient viewer. Closing Quick View or switching between Quick View and fullscreen does not dismiss or lose a persistent notification. Notifications do not intercept the viewer's navigation or exit commands.

An expected unsupported format or unavailable richer preview remains truthful inside the affected content surface and does not become an Issue merely because OneCopy has no suitable decoder. Failed requested actions and unresolved conditions share the same Issues inbox. Warning and error notifications retain their corresponding Issue even after the live notice is dismissed; informational notices do not create Issues. Reporting the same occurrence through both an Issue and a notice does not count it twice.

A broad operation such as a source check may present one summary notification while retaining the individual affected paths and explanations in Issues. Notification presentation and history remain independent of Issue dismissal.

## Issues

The Issues surface is one current-run inbox, without separate Active and Recent views or Issue-owned Retry controls. Safe recovery remains with the feature that owns it: section recheck, source checking and source repair, Background Work Resume, Managed Tools, or explicit reload/restart guidance where in-process recovery cannot be safe. Dismissing diagnostics never resumes or retries work.

Repeated background failures are combined with a count. Issue presentation must remain useful when many files fail; it summarizes the condition without hiding access to the affected files and technical context.

Retained Issue records and existing notification history are not deleted when the inbox is simplified or the app restarts. They are reconstructible library diagnostics rather than a permanent operation ledger and follow the explicit rebuild lifetime defined by `library-maintenance.md`.

## Background-worker failure

Every long-lived background worker has an outer failure boundary. An unexpected worker failure publishes a terminal failed state, releases resources it owns, records the failure when possible, and leaves an explicit Retry, Resume, or repair path. A worker must never stop while its visible state still claims that work is running or healthy.

A failure limited to one input is recorded and processing continues when later inputs remain safe and independent. Failure of shared worker state stops that worker rather than allowing it to continue with unreliable ownership or progress.

## Interface and asynchronous recovery

A drawing or rendering failure produces a visible reload or restart path instead of a blank or frozen surface. Escaped event-handler and asynchronous failures are logged and surfaced through the same notification and Issue system rather than disappearing into a console or abandoned task.

Recovery failures are themselves contained and reported. Recovery does not retry recursively or without limit. When a fallback cannot restore a safe usable state, OneCopy stops the affected surface or operation and leaves the user a direct reload, restart, retry, or repair action.

Every retryable failure identifies a reachable recovery boundary at the surviving owner; ordinary surface reopening never substitutes for explicit section recheck. A fatal startup halt names a safe next step, provides access to application logs when they can help, and retains a labelled exit. It offers in-process retry only when startup can be attempted again without bypassing or duplicating an already-admitted service.

## Restart behavior

Restart begins a fresh Issues inbox and retains prior entries with an app-restart closure reason. Explicit section recheck similarly retires the section's failed preparation/information/enrichment entries as rechecked, not as successfully repaired, before admitting another attempt. A new failure creates a fresh visible entry; merely changing or reopening a section does neither. Attempt eligibility and the preservation of successful results are owned by `library-maintenance.md`.

Restart and recheck never replay a failed or partial destructive operation. Notification history survives ordinary restart subject to its retention policy, and completed durable steps remain completed.
