# OneCopy

OneCopy is a local-first desktop app for cleaning up years of accumulated photos, videos, and other files spread across many drives. It scans the directories you point it at, collapses exact duplicates into one logical item (however many synced backup copies exist), groups near-identical spare shots by capture time and visual similarity, and lets you cull with almost nothing but the keyboard: press Enter on a photo to see its whole similar group at once — spread across every monitor you have — press the number of the one to keep, press Enter, and the rest are gone with all their copies. Kept files move out to destination folders through a verified copy pipeline that hashes while copying, retries from another copy if one has rotted, and read-back-verifies the result. It is an inbox-zero handler, not a photo manager: the goal is an empty app, with everything either deleted or moved to where it belongs.

It is built for one painful, common situation: a decade of camera rolls, camera-brand folders, and "backup before the trip" directories, kept in three or more synced copies across a shelf of hard drives, that no ordinary photo tool can dedup and cull in one pass. Deletions go to a per-drive app-managed trash by default (no OS trash size limits), so a 50,000-photo cull is reversible until you decide otherwise.

Similarity grouping and the best-shot ordering are deliberately best-effort — tuned for the "several spares of the same moment" pattern, good enough to be a game changer, and never a substitute for your eyes. Nothing is ever deleted automatically.

0.x, pre-release, under active development. macOS and Windows.

## Requirements

- To run: macOS (Apple silicon) or Windows 10/11. Extra monitors are optional but make the similar-photos comparison view considerably better.
- Video thumbnails, snapshot strips and durations use ffmpeg, which the app downloads and manages itself (free; one click in *Managed tools*). So do photos in HEIC, HEIF and AVIF; JPEG, PNG and the other everyday formats need nothing. That makes it effectively required for any library containing video, not only a phone library. Files waiting on ffmpeg show placeholders and start working the moment you install it — you are never asked to rescan. Playing a video does not need it: playback runs in the app wherever the system supports the codec, with *Open in player* as the fallback.
- To build from source: Node.js (LTS) and stable Rust.

## Download

Grab the installer or portable build for your OS from [Releases](https://github.com/nao7sep/onecopy/releases). The builds are unsigned: on macOS, right-click the app and choose **Open** the first time; on Windows, SmartScreen → **More info** → **Run anyway**. First launch opens a three-step setup: pick the directories to clean up, confirm your default timezone, choose where the preview cache lives.

## Run from source

Double-click `scripts/run-dev.command` (macOS) or run `scripts/run-dev.ps1` (Windows).

## License

MIT © 2026 Yoshinao Inoguchi

## Contact

nao7sep@gmail.com
