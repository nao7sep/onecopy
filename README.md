# OneCopy

Cull photos and videos from many drives down to one copy each, comparing similar shots side by side before you keep or delete. It scans the directories you point it at, collapses exact duplicates into one logical item, and groups near-identical spare shots by capture time and visual similarity. Similar photos can be reviewed together across the displays you choose: navigation only changes the active image, explicit keep marks form the page decision, and the visible deletion consequence is reviewed before it runs. Copy and Move write each destination file privately, read it back to verify its bytes, and publish it only after success; known filename conflicts and Move source cleanup are reviewed for the complete selection before any file is changed. OneCopy is an inbox-zero handler, not a photo manager: the goal is an empty app, with everything either deleted or delivered where it belongs.

It is built for one painful, common situation: a decade of camera rolls, camera-brand folders, and "backup before the trip" directories, kept in three or more synced copies across a shelf of hard drives, that no ordinary photo tool can dedup and cull in one pass. Recoverable deletions stay beneath the configured source or destination root whose permissions already protect those files, so a large cull remains reversible until you decide otherwise.

Similarity grouping and the best-shot ordering are deliberately best-effort — tuned for the "several spares of the same moment" pattern, good enough to be a game changer, and never a substitute for your eyes. Nothing is ever deleted automatically.

The interface speaks ten languages — English, Japanese, Chinese, Korean, Spanish, Portuguese, French, German, Italian and Russian — following the computer's language unless you choose another in Settings.

0.x, under active development. macOS and Windows.

## Requirements

- To run: macOS (Apple silicon) or Windows 10/11. Extra monitors are optional but make the similar-photos comparison view considerably better.
- Video thumbnails, snapshot strips and durations use [ffmpeg](https://ffmpeg.org/), which the app can download and manage after an explicit action in *Background work & tools*. OneCopy obtains the current Apple-silicon build from [Martin Riedl](https://ffmpeg.martin-riedl.de/) and the Windows GPL build from [BtbN](https://github.com/BtbN/FFmpeg-Builds); allow roughly 200 MiB for the download, depending on the current build and platform. So do photos in HEIC, HEIF and AVIF; JPEG, PNG and the other everyday formats need nothing. That makes ffmpeg effectively required for any library containing video, not only a phone library. Files waiting on it show truthful placeholders and become eligible when installation succeeds without requiring a source rescan. Playing a video uses the system-supported in-app codecs, with *Open in Default App* as the fallback.
- Transcription and optional face scoring are available on both Apple-silicon macOS and Windows. They use artifacts that OneCopy downloads only when you explicitly install them in *Background work & tools*: [Whisper large-v3-turbo](https://huggingface.co/ggerganov/whisper.cpp/blob/98aa99a0a9db05ae2342309f5096248665f7cba3/ggml-large-v3-turbo.bin) (about 1.51 GiB), [UltraFace RFB-640](https://github.com/onnx/models/blob/4c46cd00fbdb7cd30b6c1c17ab54f2e1f4f7b177/validated/vision/body_analysis/ultraface/models/version-RFB-640.onnx) (about 1.5 MiB), [HSEmotion EfficientNet-B2](https://github.com/sb-ai-lab/EmotiEffLib/blob/af833487321c3efdcb1768a91a6c656a1986fdf6/models/affectnet_emotions/onnx/enet_b2_8.onnx) (about 29 MiB), and, on Windows, the [official Microsoft ONNX Runtime](https://www.nuget.org/packages/Microsoft.ML.OnnxRuntime/1.28.0) (about 133 MiB to download) needed to run the face models. OneCopy pins and verifies the selected bytes; artifact updates arrive only when an app update selects a new pin. Apple-silicon macOS transcription defaults to the accepted Metal backend and Settings can switch it to CPU-only without rebuilding or restarting the app. Windows currently offers the portable CPU backend and does not claim experimental GPU acceleration. Settings derives acceleration choices from the current platform build, so future accepted backends appear only where they are actually available. The launch-time update check applies to ffmpeg, not these app-selected artifacts.
- To build from source: Node.js (LTS), stable Rust, CMake (the linked-in whisper.cpp transcription engine builds through it), and the native C/C++ build tools for your OS (Xcode Command Line Tools on macOS; Visual Studio Build Tools on Windows).

## Deleted files

Visibility filters in Settings hide items from review, not from duplicate accounting. A hidden byte-identical copy is still included when you move or delete its visible counterpart; a different hidden file is not. Visible copies supply displayed and exported filenames, while dates and copy counts still use every known copy. Changing a filter never deletes a file.

Recoverable deletion in OneCopy moves a file into a hidden `.onecopy-trash` folder beneath its most-specific configured source root; a file displaced when you approve an overwrite at a destination goes to that destination root's folder. Deleted files therefore stay on the same drive and inside the same folder permissions they came from, never in one shared drive-level or application-level bin. Each day's deletions are kept together with a record of where every file came from.

OneCopy never empties or prunes this storage on its own. The *Deleted files…* window lists the location for each configured root with its file count and size, reveals it in Finder or Explorer, and can permanently empty it only after you confirm.

*Browse…* in that window lists the stored files by day and by deleted item, so a photo comes back together with its companions, and *Restore* moves the ones you choose back to exactly where they were in their root, recreating missing folders on the way. Restore never overwrites anything: when the original place is taken, you review the conflict and the file comes back under a renamed name, and a file that changed since it was deleted can only be revealed, not restored. Restore is not Undo; it never replays or reverses an operation, and deleting a restored file again is how to take it back. Removing one of these folders yourself is also safe, because nothing outside it depends on its contents.

## Storage

OneCopy is built for local and removable drives. Before it changes files on a source drive, it checks that the drive is the one it recorded. Publishing a copy, deleting into Deleted files and restoring never replace an existing file; on exFAT, which has no exclusive rename, OneCopy reserves the name first.

What it does with storage that needs care:

- Libraries and apps (a Photos, Lightroom, Final Cut or iMovie library, an app) are left alone: OneCopy never looks inside one, and a source folder inside one is refused, because deleting or moving a "duplicate" there would damage the library.
- Online-only files in iCloud Drive, OneDrive, Dropbox and other synced folders are not read, since reading one downloads it; the source folder's Issue says how many were skipped. Make them available offline to include them. Deleting permanently or emptying Deleted files in a synced folder also deletes the files from the cloud and your other devices, and OneCopy says so before it does.
- A source folder whose drive OneCopy cannot identify (some network shares) keeps an Issue saying so: a different drive mounted in its place would not be noticed. A folder other computers write at the same time is not covered.
- Two hard links to one file count as one copy on the Mac. Windows reports links only through an open file, so there each name counts.
- Symbolic links and junctions are never followed, and nothing is deleted, moved or restored through one.
- Copy and Move keep each file's content, modified time and permissions. Finder tags, other extended attributes and resource forks are not copied. A copy onto a FAT32 or exFAT drive of a file dated before 1980 or after 2107 gets the nearest date those drives hold.
- A sparse file is written at full size at the destination, and deleting an APFS clone frees little or no space, because clones share their data.
- Destination drives are not checked for a swap. Publication never replaces a file, so a different drive mounted in the same place cannot be overwritten.

## Download

Grab the installer or portable build for your OS from [Releases](https://github.com/nao7sep/onecopy/releases/latest). The builds are unsigned: on macOS, right-click the app and choose **Open** the first time; on Windows, SmartScreen → **More info** → **Run anyway**. First launch asks for the source directories and default timezone, explains the preparation OneCopy always needs, and lets you review optional background analysis. Managed tools and models are never downloaded merely because an optional feature is enabled.

## Run from source

For a production-faithful compiled build, double-click `scripts/rebuild.command` on macOS or run `scripts/rebuild.ps1` on Windows. Once built, `scripts/run-built.command` / `scripts/run-built.ps1` launches the existing binary without rebuilding. For live-reload development, use `scripts/run-dev.command` / `scripts/run-dev.ps1`.

By hand, install the locked packages with `npm ci`, run checks with `npm test`, and build the packaged app with `npm run tauri build`.

## Tests

`npm test` runs the same fixed set every time: the source-text check, the type check over the application and test code, the production frontend build, every frontend test, and every Rust test. `npm run test:full` runs all of that and adds the slower heavy suite; run it at the end of a batch of changes and before a release, on macOS and on Windows. The heavy suite is face scoring, transcription, still and video preparation, and Live Photo metadata through the real managed ffmpeg and models, over the shared test-fixture corpus in the company repository, which must be checked out beside this one. The first full run downloads the managed tools and models (about 1.7 GB, mostly the Whisper model) through the app's own acquisition code and caches them in the Rust build directory; later runs download again only what the app itself would update. App-level acceptance is performed by using the real built application with a disposable app home and a disposable copy of the shared test fixtures; destructive testing never targets the shared fixture directory itself.

## License

[GNU GPL v3 or later](LICENSE) © 2026 Yoshinao Inoguchi

## Contact

Yoshinao Inoguchi — yoshinao@inoguchi.com — <https://inoguchi.com>
