# OneCopy

OneCopy is a local-first desktop app for cleaning up years of accumulated photos, videos, and other files spread across many drives. It scans the directories you point it at, collapses exact duplicates into one logical item (however many synced backup copies exist), groups near-identical spare shots by capture time and visual similarity, and lets you cull with almost nothing but the keyboard: press Enter on a photo to see its whole similar group at once — spread across every monitor you have — press the number of the one to keep, press Enter, and the rest are gone with all their copies. Kept files move out to destination folders one output at a time: OneCopy writes each output privately, reads it back and verifies it, then publishes it without overwriting an existing file. If one known source cannot be read, it can try another. It is an inbox-zero handler, not a photo manager: the goal is an empty app, with everything either deleted or moved to where it belongs.

It is built for one painful, common situation: a decade of camera rolls, camera-brand folders, and "backup before the trip" directories, kept in three or more synced copies across a shelf of hard drives, that no ordinary photo tool can dedup and cull in one pass. Deletions go to a per-drive app-managed trash by default (no OS trash size limits), so a 50,000-photo cull is reversible until you decide otherwise.

Similarity grouping and the best-shot ordering are deliberately best-effort — tuned for the "several spares of the same moment" pattern, good enough to be a game changer, and never a substitute for your eyes. Nothing is ever deleted automatically.

0.x, under active development. macOS and Windows.

## Requirements

- To run: macOS (Apple silicon) or Windows 10/11. Extra monitors are optional but make the similar-photos comparison view considerably better.
- Video thumbnails, snapshot strips and durations use ffmpeg, which the app downloads and manages itself (free; one click in *Managed tools*). So do photos in HEIC, HEIF and AVIF; JPEG, PNG and the other everyday formats need nothing. That makes it effectively required for any library containing video, not only a phone library. Files waiting on ffmpeg show placeholders and start working the moment you install it — you are never asked to rescan. Playing a video does not need it: playback runs in the app wherever the system supports the codec, with *Open in player* as the fallback.
- Transcription and optional face scoring use models that OneCopy downloads only when you explicitly install them in *Managed tools*: Whisper large-v3-turbo (about 1.51 GiB, from the whisper.cpp model repository), UltraFace RFB-640 (about 1.5 MiB, from the ONNX Model Zoo), and HSEmotion EfficientNet-B2 (about 29 MiB, from EmotiEffLib). OneCopy pins and verifies the selected bytes; model updates arrive only when an app update selects a new pin. The launch-time update check applies to ffmpeg, not these app-selected models.
- To build from source: Node.js (LTS), stable Rust, CMake (the linked-in whisper.cpp transcription engine builds through it), and the native C/C++ build tools for your OS (Xcode Command Line Tools on macOS; Visual Studio Build Tools on Windows).

## Trash

Deleting in OneCopy moves files to an app-managed trash, not the OS one (OS trashes have size caps and quiet eviction). One trash lives at the root of each drive — `.onecopy-trash`, hidden — so a delete is an instant same-drive rename; files from your system drive go to the app's own folder instead (`~/.onecopy/trash` on macOS). Inside, each UTC day gets a folder holding that day's deletions flat, plus a `manifest.jsonl` recording every file's original path, stored name, and content hash.

The trash is write-only by design: **the app never empties or prunes it on its own** — nothing leaves a trash except by your explicit hand. The *Trash…* window lists every trash on every attached drive (including drives you no longer scan), shows exact file counts and sizes, reveals any of them in the file manager, and can empty one after confirming precisely what will be destroyed. Recovery is manual in Finder or Explorer; OneCopy does not provide a Restore command. Deleting a trash folder yourself is also always safe — nothing in the app depends on its contents.

## Download

Grab the installer or portable build for your OS from [Releases](https://github.com/nao7sep/onecopy/releases/latest). The builds are unsigned: on macOS, right-click the app and choose **Open** the first time; on Windows, SmartScreen → **More info** → **Run anyway**. First launch opens a two-step setup: pick the directories to clean up and confirm your default timezone.

## Run from source

For a production-faithful compiled build, double-click `scripts/rebuild.command` on macOS or run `scripts/rebuild.ps1` on Windows. Once built, `scripts/run-built.command` / `scripts/run-built.ps1` launches the existing binary without rebuilding. For live-reload development, use `scripts/run-dev.command` / `scripts/run-dev.ps1`.

By hand, install the locked packages with `npm ci`, run checks with `npm run check`, and build the packaged app with `npm run tauri build`.

## License

MIT © 2026 Yoshinao Inoguchi

## Contact

Yoshinao Inoguchi — yoshinao@inoguchi.com — <https://inoguchi.com>
