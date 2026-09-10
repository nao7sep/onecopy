# Release Checking

## Scope and controls

- OneCopy checks only the latest published full release in its fixed public GitHub repository. It never retrieves release notes, downloads or installs assets, restarts OneCopy, or alters the release process.
- About exposes **Check GitHub for New Release**. Settings exposes **Check GitHub for new releases at launch**, which defaults on and preserves an explicitly saved false value.
- **View Release on GitHub** opens the repository's fixed latest-release page only after the user invokes it.

## Request and comparison

- One asynchronous unauthenticated request reads only `tag_name` from GitHub's fixed latest-release API endpoint. It sends GitHub's JSON accept and current API-version headers plus a OneCopy user agent; it sends no token, user data, machine identifier, or telemetry.
- The whole request is bounded to ten seconds, runs away from the interface thread, and is never retried. A non-success status, timeout, rate limit, malformed JSON, missing tag, or tag outside the accepted `vX.Y.Z` form is a failed check.
- The installed version comes from OneCopy's canonical package version. Versions compare by semantic precedence. Equal or older published versions are current, including when a development build is newer.

## Lifecycle and presentation

- A launch check begins only after saved configuration and Main's usable interface are ready, never delays startup, and runs at most once in a process. It is eligible when the app-release last-attempt timestamp is missing, invalid, in the future, or at least 24 hours old.
- Record the app-release UTC last-attempt timestamp immediately before every automatic or manual request. Persist no result, tag, retry time, response body, ETag, or rate-limit state.
- At most one app-release request is in flight. A manual action during a launch check joins and visibly presents that result; otherwise manual checks bypass the 24-hour interval.
- Automatic current and failed outcomes are quiet; failures remain in the diagnostic log. A newer release creates one dismissible nonmodal notice naming the version and offering **View Release on GitHub**. Manual checks visibly report newer, current, or failed outcomes at About.
- App-release checking remains independent from Managed Tools: the two features have distinct preferences, timestamps, commands, results, and in-flight ownership, and neither waits for or interprets the other.
