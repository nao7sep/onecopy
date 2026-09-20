# Interface Language

## Language set and choice

- OneCopy's interface is available in ten languages: English, Japanese, Chinese, Korean, Spanish, Portuguese, French, German, Italian and Russian. Chinese is Simplified Chinese and Portuguese is Brazilian Portuguese; each language is named in its own words. English is the source language and the fallback for every computer whose language is outside the set.
- Settings exposes **System** followed by each language under its own name. **System** resolves at every launch from the computer's preferred languages in order, taking the first one in the set: every Chinese locale resolves to Chinese, every Portuguese locale to Portuguese, and every Spanish locale to Spanish.
- The first-launch wizard exposes the same choice and opens in the resolved **System** language. A language chosen there takes effect when the wizard finishes, with its other answers.
- A chosen language is durable. It survives a change of the computer's language and stays until the user changes it. A missing or unrecognized saved value means **System**.
- The language applies on Save with its neighbouring settings and needs no restart: every open OneCopy window follows it, including windows that are hidden or reused.

## Surfaces

- Every window OneCopy draws speaks the interface language, including its title: Main, Preview, Viewer, Comparison and the Comparison image windows.
- The native menu speaks it. The items macOS contributes itself, such as Services, Emoji & Symbols and Start Dictation, follow at the next launch.
- Text shown before OneCopy can read a saved language — the launch-failure dialog — uses the computer's language.
- The first text drawn in a window is already in the interface language; no window shows English first.
- Dates, times, numbers and sizes are formatted for the interface language rather than one fixed English form.

## Text that stays as recorded

- Internal and key-like text is not translated: condition codes and identifiers, diagnostic detail, log files, and the names and paths of the user's own files and folders.
- A notice or an Issue appears in the interface language when its condition identifies what to say. Detail OneCopy cannot restate, such as a system or managed-tool error, appears as recorded, after the translated sentence, whatever language that detail is in.
- An Issue recorded before the language changed keeps its recorded detail; only the part OneCopy supplies follows the current language.
- Changing the language never rewrites stored records, user files, or the library.

## Boundaries

- Transcription detects spoken language automatically and gains no language selector from this contract, per `library-maintenance.md`.
- The configured default timezone stays with `library-items.md`: it is chosen from a list, defaults to the computer's zone at first launch, and does not follow later changes of the computer's zone.
