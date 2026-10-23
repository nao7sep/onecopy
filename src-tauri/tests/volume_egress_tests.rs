// Keeps filesystem calls on user volumes inside the bounded owner
// (`volume_io`). A filesystem call has no native timeout, so every call on a
// configured source or destination runs through the one owner that can give
// up on it when a drive stops responding; this test enforces that in code
// rather than by review.
//
// A shipped source file may name a raw filesystem primitive only when:
// - it is `volume_io.rs`, the owner itself;
// - it is a module that touches only OneCopy's own data folder, listed below
//   with why: that folder is on the local disk, so a stalled removable or
//   network drive cannot hold its calls;
// - the line carries `// data root` (a mixed module's cache or temp access) or
//   `// volume_io worker` (code that runs inside a `volume_io` call).

use std::path::Path;

/// Modules whose filesystem access is only OneCopy's own data folder.
const DATA_ROOT_MODULES: &[(&str, &str)] = &[
    ("ai_dependencies.rs", "managed model files"),
    ("backup_store.rs", "managed-text backups"),
    ("binaries.rs", "managed tools"),
    ("binaries_acquisition.rs", "managed tool downloads"),
    ("binaries_manager.rs", "managed tools"),
    ("face.rs", "preview cache and face models"),
    ("index_store.rs", "index database"),
    ("instance_owner.rs", "instance lock"),
    ("logging.rs", "records fallback log files"),
    ("paths.rs", "data folder layout"),
    ("records.rs", "records database"),
    ("sqlite.rs", "index database"),
    ("storage.rs", "config and state files"),
    ("transcription.rs", "transcription temp audio"),
    ("viewer_sequence.rs", "viewer temp files"),
];

const MARKERS: &[&str] = &["// data root", "// volume_io worker"];

/// A raw filesystem primitive, as it appears in source. `prev` is the
/// previous line's code, used when a chained call's receiver sits at the end
/// of the line above (`reader\n    .metadata()`).
fn raw_primitive(code: &str, prev: &str) -> Option<&'static str> {
    // An extern declaration names a symbol but performs no filesystem work.
    if code.trim_start().starts_with("fn ") && code.trim_end().ends_with(';') {
        return None;
    }
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
        "libc::renameatx_np(",
        "libc::openat(",
        "libc::fstatfs(",
        "DirBuilder::new(",
        "acl_get_fd(",
        "acl_set_fd(",
        "CreateDirectoryW(",
        "GetVolumeInformationByHandleW(",
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

/// One raw filesystem call found in a source file.
struct RawCall<'a> {
    line_number: usize,
    primitive: &'static str,
    line: &'a str,
    /// The line, or the one above it, carries one of `MARKERS`.
    marked: bool,
}

/// Every raw filesystem call in `text`, whole-line comments skipped and
/// trailing comments ignored. Both the violation scan and the stale-entry
/// check read source through this, so they cannot disagree on what a raw call
/// is.
fn raw_calls(text: &str) -> Vec<RawCall<'_>> {
    let lines: Vec<&str> = text.lines().collect();
    let mut calls = Vec::new();
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
        calls.push(RawCall {
            line_number: index + 1,
            primitive,
            line: line.trim(),
            marked,
        });
    }
    calls
}

/// Listed data-folder modules whose exemption no longer matches anything:
/// the file is gone, or it no longer makes a raw filesystem call. `read`
/// returns a module's source, or `None` when the file does not exist.
fn stale_data_root_modules(
    modules: &[(&str, &str)],
    read: impl Fn(&str) -> Option<String>,
) -> Vec<String> {
    modules
        .iter()
        .filter_map(|(module, _)| match read(module) {
            None => Some(format!("{module} is listed but gone")),
            Some(text) if raw_calls(&text).is_empty() => Some(format!(
                "{module} is listed but makes no raw filesystem call"
            )),
            Some(_) => None,
        })
        .collect()
}

fn source_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

#[test]
fn only_the_bounded_owner_touches_user_volumes() {
    let mut offenders = Vec::new();
    let mut files: Vec<_> = walkdir::WalkDir::new(source_dir())
        .into_iter()
        .map(|entry| entry.unwrap().into_path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .collect();
    files.sort();
    for path in files {
        let name = path.strip_prefix(source_dir()).unwrap().to_string_lossy().into_owned();
        if name == "volume_io.rs" || DATA_ROOT_MODULES.iter().any(|(module, _)| *module == name) {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for call in raw_calls(&text) {
            if !call.marked {
                offenders.push(format!(
                    "{name}:{}: {}: {}",
                    call.line_number, call.primitive, call.line
                ));
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
fn every_listed_data_root_module_still_makes_a_raw_call() {
    let stale = stale_data_root_modules(DATA_ROOT_MODULES, |module| {
        std::fs::read_to_string(source_dir().join(module)).ok()
    });
    assert!(
        stale.is_empty(),
        "remove these from DATA_ROOT_MODULES:\n{}",
        stale.join("\n")
    );
}

#[test]
fn the_stale_check_flags_a_listed_module_without_raw_calls() {
    // Planted modules: one that still reads a file, one whose only filesystem
    // mentions are a comment and an accessor on metadata already read, and
    // one whose file is gone.
    let modules = [
        ("reads.rs", "still reads"),
        ("no_calls.rs", "reads nothing now"),
        ("gone.rs", "deleted"),
    ];
    let stale = stale_data_root_modules(&modules, |module| match module {
        "reads.rs" => Some("let text = std::fs::read_to_string(path)?;".to_owned()),
        "no_calls.rs" => Some(
            "// std::fs::read(path) used to live here\nif metadata.is_dir() {}".to_owned(),
        ),
        _ => None,
    });
    assert_eq!(
        stale,
        [
            "no_calls.rs is listed but makes no raw filesystem call",
            "gone.rs is listed but gone",
        ]
    );
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
        "let moved = unsafe { libc::renameatx_np(fd, from, fd, to, libc::RENAME_EXCL) };",
        "let acl = unsafe { acl_get_fd(fd) };",
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
        "fn acl_get_fd(fd: i32) -> *mut c_void;",
    ] {
        assert!(raw_primitive(accessor, "").is_none(), "{accessor}");
    }
    assert!(
        raw_primitive(".metadata()", "let expected_total = reader").is_none(),
        "a chained call on a known-safe receiver from the line above is not flagged"
    );
}
