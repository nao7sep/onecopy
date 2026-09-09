# Library Visibility

## Policy and scope

One shared durable Settings policy determines which source items and destination descendants appear in ordinary browsing. The ignored-name list matches complete file basenames without regard to case, not directory names or glob expressions. Its defaults are .DS_Store, Thumbs.db, and desktop.ini. Users may add, edit, or remove entries; an empty list ignores no names.

Separate default-on choices hide dot-prefixed files and directories, files and directories carrying native hidden attributes, and native system attributes where the platform supports them. Platform-specific controls describe actual platform capabilities. Directory hiding applies to its descendants. Explicitly configured roots remain reachable; filtering operates beneath those roots rather than hiding a root because of its own name or ancestors.

OneCopy deleted-file storage remains unconditionally excluded under `file-operations.md`, independent of these settings. A similarly named ordinary directory is not deleted-file storage.

## Inventory and logical review eligibility

Visibility is a presentation policy, not permission to discard source records or an exemption from duplicate cleanup. Source discovery and file-information completion retain hidden files beneath configured roots and may establish their byte identity and companion relationships. They never search outside those roots to find additional copies.

A logical item appears in ordinary review if at least one currently available main copy passes the policy. Hidden-only logical items and sections with no eligible items do not appear. Copy counts and physical-copy details continue to describe all known available copies, including hidden copies of a visible logical item. Representative naming and date evidence follow `library-items.md`; confirmed filesystem effects follow `file-operations.md`.

## Changes and reconciliation

Changing visibility does not delete files, discard indexed identity or date evidence, erase completed preparation, retry failed work, or broaden a frozen file-operation plan. It republishes review eligibility and representative names from known facts. Lifting a filter makes eligible content available again without rebuilding the library.

Main preserves surviving selection and anchor by identity and applies its ordinary disappearance recovery when an item becomes hidden-only. Active frozen viewing and Comparison sessions may remove ineligible members but never acquire newly revealed members outside their original membership. Diagnostic navigation cannot bypass review eligibility.

Destination listing and disclosure use the same policy. A folder with no visible children has no expansion affordance, but a folder containing hidden files is not physically empty. Changing filters invalidates cached destination children and removes a selected destination that is no longer reachable in the visible tree.
