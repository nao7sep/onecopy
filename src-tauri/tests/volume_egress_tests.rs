// Keeps filesystem calls on user volumes inside the bounded owner
// (`volume_io`). PLAYBOOK "Bound every external wait": a filesystem call has
// no native timeout, so every call on a configured source or destination runs
// through the one owner that can give up on it; this test is that rule's
// enforcement in code rather than by review.
//
// A shipped source file may name a raw filesystem primitive only when:
// - it is `volume_io.rs`, the owner itself;
// - it is a module that touches only OneCopy's own data folder (assumed local
//   and not routed through the owner — plan decision), listed below with why;
// - the line carries `// data root` (a mixed module's cache or temp access) or
//   `// volume_io worker` (code that runs inside a `volume_io` call).

use std::path::Path;

/// Modules whose filesystem access is only OneCopy's own data folder.
const DATA_ROOT_MODULES: &[(&str, &str)] = &[
    ("ai_acceleration.rs", "managed runtime files"),
    ("ai_dependencies.rs", "managed model files"),
    ("backup_store.rs", "managed-text backups"),
    ("binaries.rs", "managed tools"),
    ("binaries_acquisition.rs", "managed tool downloads"),
    ("binaries_manager.rs", "managed tools"),
    ("face.rs", "preview cache and face models"),
    ("github_release.rs", "release-check state"),
    ("i18n.rs", "config language"),
    ("index_store.rs", "index database"),
    ("instance_owner.rs", "instance lock"),
    ("logging.rs", "records fallback log files"),
    ("paths.rs", "data folder layout"),
    ("records.rs", "records database"),
    ("sqlite.rs", "index database"),
    ("startup.rs", "data folder preparation"),
    ("storage.rs", "config and state files"),
    ("theme.rs", "config theme"),
    ("transcription.rs", "transcription temp audio"),
    ("viewer_sequence.rs", "viewer temp files"),
    ("window_placement.rs", "window state"),
];

const MARKERS: &[&str] = &["// data root", "// volume_io worker"];

/// A raw filesystem primitive, as it appears in source. `prev` is the
/// previous line's code, used when a chained call's receiver sits at the end
/// of the line above (`reader\n    .metadata()`).
fn raw_primitive(code: &str, prev: &str) -> Option<&'static str> {
    const NEEDLES: &[&str] = &[
        "File::open(",
        "File::create(",
        "File::options(",
        "OpenOptions::new(",
        "walkdir::",
        "WalkDir",
        "ImageReader::open(",
        "image::open(",
        "nom_exif::read_exif(",
        "nom_exif::read_exif_iter(",
        "nom_exif::read_track(",
        "nom_exif::read_metadata(",
        "MediaSource::open(",
        "recommended_watcher(",
        ".canonicalize()",
        ".exists()",
        ".try_exists()",
        ".read_dir()",
        ".symlink_metadata()",
        // Native filesystem calls, which no Rust lint sees as file access.
        "libc::renamex_np(",
        "libc::rename(",
        "libc::open(",
        "libc::stat(",
        "libc::lstat(",
        "libc::statfs(",
        "libc::pathconf(",
        "libc::unlink(",
        "MoveFileExW(",
        "CreateFileW(",
        "DeleteFileW(",
        "RemoveDirectoryW(",
        "GetFileAttributesW(",
        "SetFileAttributesW(",
        "GetVolumeInformationW(",
    ];
    if let Some(needle) = NEEDLES.iter().find(|needle| code.contains(**needle)) {
        return Some(needle);
    }
    // `std::fs::read(…)`, `fs::metadata(…)`: a free function of `std::fs`.
    // Type names (`std::fs::File`, `std::fs::Metadata`) are capitalized.
    let mut rest = code;
    while let Some(index) = rest.find("fs::") {
        let before = rest[..index].chars().last();
        let after = &rest[index + 4..];
        let name: String = after
            .chars()
            .take_while(|c| c.is_ascii_lowercase() || *c == '_')
            .collect();
        if !before.is_some_and(|c| c.is_alphanumeric() || c == '_')
            && !name.is_empty()
            && after[name.len()..].trim_start().starts_with('(')
        {
            return Some("std::fs function");
        }
        rest = after;
    }
    // A path probe, as opposed to the same-named accessor on a `FileType` or
    // `Metadata` value already read.
    for probe in [".is_dir()", ".is_file()"] {
        let mut rest = code;
        while let Some(index) = rest.find(probe) {
            let receiver = rest[..index]
                .trim_end()
                .rsplit(|c: char| !(c.is_alphanumeric() || c == '_' || c == '(' || c == ')' || c == '.'))
                .next()
                .unwrap_or("");
            let last = receiver.rsplit('.').next().unwrap_or(receiver);
            if !matches!(
                last,
                "file_type()" | "file_type" | "metadata" | "meta" | "m" | "kind"
            ) {
                return Some("path probe");
            }
            rest = &rest[index + probe.len()..];
        }
    }
    // `Path::metadata()`/`symlink_metadata()`-shaped stats, as opposed to the
    // same-named call on a file handle already open through `volume_io`.
    {
        const SAFE_FILE_RECEIVERS: &[&str] = &["file", "reader", "writer", "source_file"];
        let mut rest = code;
        while let Some(index) = rest.find(".metadata()") {
            let mut receiver = rest[..index]
                .trim_end()
                .rsplit(|c: char| !(c.is_alphanumeric() || c == '_' || c == '(' || c == ')' || c == '.'))
                .next()
                .unwrap_or("");
            if receiver.is_empty() {
                receiver = prev
                    .trim_end()
                    .rsplit(|c: char| !(c.is_alphanumeric() || c == '_' || c == '(' || c == ')' || c == '.'))
                    .next()
                    .unwrap_or("");
            }
            let last = receiver.rsplit('.').next().unwrap_or(receiver);
            if !SAFE_FILE_RECEIVERS.contains(&last) {
                return Some("Path::metadata()");
            }
            rest = &rest[index + ".metadata()".len()..];
        }
    }
    None
}

fn source_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

#[test]
fn only_the_bounded_owner_touches_user_volumes() {
    let mut offenders = Vec::new();
    let mut files: Vec<_> = std::fs::read_dir(source_dir())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .collect();
    files.sort();
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name == "volume_io.rs" || DATA_ROOT_MODULES.iter().any(|(module, _)| *module == name) {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            let code = line.split("//").next().unwrap_or("");
            let prev = if index > 0 {
                lines[index - 1].split("//").next().unwrap_or("")
            } else {
                ""
            };
            let Some(primitive) = raw_primitive(code, prev) else {
                continue;
            };
            let marked = MARKERS.iter().any(|marker| {
                line.contains(marker) || index > 0 && lines[index - 1].contains(marker)
            });
            if !marked {
                offenders.push(format!("{name}:{}: {primitive}: {}", index + 1, line.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "filesystem calls outside volume_io (route them through it, or mark a data-folder \
         line `// data root`):\n{}",
        offenders.join("\n")
    );
}

#[test]
fn every_listed_data_root_module_still_exists() {
    for (module, _) in DATA_ROOT_MODULES {
        assert!(source_dir().join(module).is_file(), "{module} is listed but gone");
    }
}

#[test]
fn the_lint_recognizes_raw_calls_and_ignores_accessors() {
    for raw in [
        "let bytes = std::fs::read(path)?;",
        "if path.is_dir() {",
        "for entry in walkdir::WalkDir::new(root) {",
        "let file = File::open(path)?;",
        "if !target.exists() {",
        "let exif = nom_exif::read_exif(path)?;",
        "let source = nom_exif::MediaSource::open(path)?;",
        "let existing = unsafe { GetFileAttributesW(wide.as_ptr()) };",
        "let moved = unsafe { libc::renamex_np(from, to, libc::RENAME_EXCL) };",
        "let meta = path.metadata()?;",
        "let meta = entry.metadata()?;",
    ] {
        assert!(raw_primitive(raw, "").is_some(), "{raw}");
    }
    assert!(
        raw_primitive(".metadata()", "let total = tree").is_some(),
        "a chained call on a receiver from the line above must still be seen"
    );
    for accessor in [
        "if entry.file_type().is_file() {",
        "if metadata.is_dir() {",
        "fn check(meta: &std::fs::Metadata) {",
        "use std::os::unix::fs::MetadataExt;",
        "crate::volume_io::metadata(path)?",
        "let len = file.metadata()?.len();",
        "let len = reader.metadata()?.len();",
    ] {
        assert!(raw_primitive(accessor, "").is_none(), "{accessor}");
    }
    assert!(
        raw_primitive(".metadata()", "let expected_total = reader").is_none(),
        "a chained call on a known-safe receiver from the line above is not flagged"
    );
}
