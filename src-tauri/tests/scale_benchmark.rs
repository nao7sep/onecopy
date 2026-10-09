// The scale benchmark: builds a disposable library and times the operations
// OneCopy's speed targets are about. It is not part of the default tests:
//
//   cargo test --release --test scale_benchmark -- --ignored --nocapture
//
// ONECOPY_BENCH_FILES sets the library size (default 50000, of which three
// sections of 10,000 photos, videos and other files share one month, and the
// rest spread over a year); a tenth of the photos also have a copy in a second
// folder. ONECOPY_BENCH_DIR names the folder to build it in (default: a
// temporary folder, removed afterwards). Videos are placeholder bytes, so their
// metadata reads fail as they would for damaged files; walking, hashing and
// the index are what this measures, not media decoding.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use chrono::TimeZone;
use chrono_tz::Tz;
use onecopy_lib::{derived_state, index_store, operations, preview::CachePaths, queries, scanner};
use rusqlite::Connection;

const SECTION_MONTH: &str = "2024-01";

struct Library {
    _dir: Option<tempfile::TempDir>,
    home: PathBuf,
    source: PathBuf,
    files: usize,
    section: usize,
}

fn month_time(year: i32, month: u32, offset: u32) -> SystemTime {
    let at = chrono::Utc.with_ymd_and_hms(year, month, 1 + offset % 27, 12, 0, 0).unwrap();
    SystemTime::UNIX_EPOCH + Duration::from_secs(at.timestamp() as u64)
}

fn write_file(path: &Path, bytes: &[u8], modified: SystemTime) {
    std::fs::write(path, bytes).unwrap();
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
}

fn base_jpeg() -> Vec<u8> {
    let image = image::RgbImage::from_fn(64, 48, |x, y| image::Rgb([(x * 4) as u8, (y * 5) as u8, 128]));
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Jpeg)
        .unwrap();
    bytes
}

fn build_library(files: usize) -> Library {
    let (dir, root) = match std::env::var_os("ONECOPY_BENCH_DIR") {
        Some(dir) => (None, PathBuf::from(dir)),
        None => {
            let dir = tempfile::Builder::new().prefix("onecopy-scale-").tempdir().unwrap();
            let root = dir.path().to_path_buf();
            (Some(dir), root)
        }
    };
    let home = root.join("home");
    let source = root.join("library");
    std::fs::create_dir_all(&home).unwrap();
    let jpeg = base_jpeg();
    let section = (files / 5).min(10_000);
    let mut written = 0usize;
    let unique = |base: &[u8], index: usize| {
        let mut bytes = base.to_vec();
        bytes.extend_from_slice(format!("onecopy-bench-{index}").as_bytes());
        bytes
    };
    for (kind, folder, ext, base) in [
        ("photo", "Photos/2024-01", "jpg", jpeg.as_slice()),
        ("video", "Videos/2024-01", "mp4", b"\0\0\0\x18ftypisom".as_slice()),
        ("other", "Documents/2024-01", "txt", b"notes ".as_slice()),
    ] {
        let dir = source.join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..section {
            let name = format!("{kind}_{i:06}.{ext}");
            write_file(&dir.join(name), &unique(base, written), month_time(2024, 1, i as u32));
            written += 1;
        }
    }
    let backup = source.join("Backup/2024-01");
    std::fs::create_dir_all(&backup).unwrap();
    for i in (0..section).step_by(10) {
        let name = format!("photo_{i:06}.jpg");
        std::fs::copy(source.join("Photos/2024-01").join(&name), backup.join(&name)).unwrap();
        written += 1;
    }
    let mut month = 0u32;
    while written < files {
        let dir = source.join(format!("Archive/2023-{:02}", month % 12 + 1));
        std::fs::create_dir_all(&dir).unwrap();
        let name = format!("photo_{written:06}.jpg");
        write_file(&dir.join(name), &unique(&jpeg, written), month_time(2023, month % 12 + 1, written as u32));
        written += 1;
        month += 1;
    }
    Library { _dir: dir, home, source, files: written, section }
}

fn scan_settings(source: &Path, home: &Path) -> scanner::ScanSettings {
    let config = serde_json::json!({
        "sourceDirs": [source.to_string_lossy()],
        "defaultTimezone": "UTC",
    });
    scanner::settings_from_config(Some(&config), home, chrono::Utc::now().timestamp_millis())
}

fn full_scan(conn: &Connection, settings: &scanner::ScanSettings) -> (Duration, Duration) {
    full_scan_phases(conn, settings).0
}

/// A full scan, with the time each phase started (from the progress events).
fn full_scan_phases(
    conn: &Connection,
    settings: &scanner::ScanSettings,
) -> ((Duration, Duration), Vec<(scanner::ScanPhase, Duration)>) {
    let started = Instant::now();
    let phases = std::cell::RefCell::new(Vec::<(scanner::ScanPhase, Duration)>::new());
    let note = |progress: scanner::ScanProgress| {
        let mut phases = phases.borrow_mut();
        if phases.last().map(|(phase, _)| *phase) != Some(progress.phase) {
            phases.push((progress.phase, started.elapsed()));
        }
    };
    let mut summary = scanner::run_source_check(conn, settings, &note).unwrap();
    let walked = started.elapsed();
    scanner::run_index_tail(conn, settings, &note, &mut summary).unwrap();
    let total = started.elapsed();
    let mut phases = phases.into_inner();
    phases.push((scanner::ScanPhase::Indexed, total));
    ((walked, total), phases)
}

fn report_phases(label: &str, phases: &[(scanner::ScanPhase, Duration)]) {
    for pair in phases.windows(2) {
        report(&format!("  {label}: {:?}", pair[0].0), pair[1].1 - pair[0].1);
    }
}

fn projection() -> queries::ItemProjectionContext {
    queries::ItemProjectionContext {
        capabilities: derived_state::WorkCapabilities {
            ffmpeg: true,
            video_snapshots_enabled: true,
            similarity_enabled: true,
            face_enabled: false,
            face_models: false,
            transcription_model: false,
            transcription_acceleration: true,
            face_acceleration: true,
            video_transcription_enabled: true,
            audio_transcription_enabled: true,
        },
    }
}

fn open_section(conn: &Connection, kind: queries::SectionKind, start: u64) -> usize {
    queries::section_window(
        conn,
        kind,
        SECTION_MONTH,
        Tz::UTC,
        queries::SectionSort { order: queries::SectionSortOrder::Time, desc: false },
        start,
        queries::MAX_SECTION_WINDOW_ITEMS,
        projection(),
    )
    .unwrap()
    .items
    .len()
}

fn report(label: &str, elapsed: Duration) {
    println!("{label:<44} {:>10.1} ms", elapsed.as_secs_f64() * 1000.0);
}

fn percentile(sorted: &[Duration], fraction: f64) -> Duration {
    sorted[((sorted.len() - 1) as f64 * fraction).round() as usize]
}

#[test]
#[ignore = "scale benchmark: builds a large library; run explicitly with --ignored"]
fn scale_benchmark() {
    let files = std::env::var("ONECOPY_BENCH_FILES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(50_000);
    let started = Instant::now();
    let library = build_library(files);
    println!("library: {} files in {}", library.files, library.source.display());
    report("build library (not a target)", started.elapsed());
    let settings = scan_settings(&library.source, &library.home);
    let db = library.home.join("index.sqlite3");
    let cache = CachePaths::new(settings.cache_root.clone());

    let conn = index_store::open(&db).unwrap();
    let ((walked, scanned), phases) = full_scan_phases(&conn, &settings);
    report("first scan: source check (walk)", walked);
    report("first scan: walk + file information", scanned);
    report_phases("first scan", &phases);
    let ((walked, scanned), phases) = full_scan_phases(&conn, &settings);
    report("repeat scan: source check (walk)", walked);
    report("repeat scan: walk + file information", scanned);
    report_phases("repeat scan", &phases);
    drop(conn);

    let started = Instant::now();
    let conn = index_store::open(&db).unwrap();
    queries::section_counts(&conn, Tz::UTC).unwrap();
    open_section(&conn, queries::SectionKind::Image, 0);
    report("launch: open index, counts, first section", started.elapsed());

    for (label, kind) in [
        ("photos", queries::SectionKind::Image),
        ("videos", queries::SectionKind::Video),
        ("other files", queries::SectionKind::Other),
    ] {
        let started = Instant::now();
        let shown = open_section(&conn, kind, 0);
        report(&format!("section open: {} {label} ({shown} shown)", library.section), started.elapsed());
    }

    // Scrolling while a scan writes: windows down the photo section, read on
    // this connection while another connection runs a full scan of newly added
    // files.
    let added = library.source.join("Incoming/2024-01");
    std::fs::create_dir_all(&added).unwrap();
    let jpeg = base_jpeg();
    for i in 0..2_000 {
        let mut bytes = jpeg.clone();
        bytes.extend_from_slice(format!("incoming-{i}").as_bytes());
        write_file(&added.join(format!("new_{i:06}.jpg")), &bytes, month_time(2024, 1, i));
    }
    let scan_db = db.clone();
    let (scan_source, scan_home) = (library.source.clone(), library.home.clone());
    let scanning = std::thread::spawn(move || {
        let conn = index_store::open(&scan_db).unwrap();
        full_scan(&conn, &scan_settings(&scan_source, &scan_home)).1
    });
    let mut latencies = Vec::new();
    let mut start = 0u64;
    while !scanning.is_finished() {
        let began = Instant::now();
        let shown = open_section(&conn, queries::SectionKind::Image, start) as u64;
        latencies.push(began.elapsed());
        start = if shown == 0 { 0 } else { start + shown / 2 };
    }
    let scan_time = scanning.join().unwrap();
    latencies.sort();
    report("scan of 2,000 new photos (writer)", scan_time);
    if !latencies.is_empty() {
        report(&format!("scrolling during scan: median of {}", latencies.len()), percentile(&latencies, 0.5));
        report("scrolling during scan: 95th percentile", percentile(&latencies, 0.95));
        report("scrolling during scan: worst", *latencies.last().unwrap());
    }

    for count in [1usize, 1_000] {
        let items: Vec<operations::ItemIdentity> = conn
            .prepare(
                "SELECT content_hash, id FROM paths WHERE kind = 'other' AND missing = 0 \
                 AND companion_of IS NULL ORDER BY id LIMIT ?1",
            )
            .unwrap()
            .query_map([count as i64], |row| {
                Ok(match row.get::<_, Option<String>>(0)? {
                    Some(hash) => operations::ItemIdentity { hash: Some(hash), path_id: None },
                    None => operations::ItemIdentity { hash: None, path_id: Some(row.get(1)?) },
                })
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let started = Instant::now();
        let mut first_delete = None;
        operations::delete_batch(
            &conn,
            &library.home,
            &cache,
            &items,
            operations::DeleteMode::Trash,
            &|| false,
            |progress| {
                if first_delete.is_none()
                    && matches!(progress, operations::DeleteBatchProgress::Deleting { files_done, .. } if files_done > 0)
                {
                    first_delete = Some(started.elapsed());
                }
            },
        )
        .unwrap();
        let total = started.elapsed();
        report(&format!("delete start: first of {count} in Deleted files"), first_delete.unwrap_or(total));
        report(&format!("delete: all {count}"), total);
    }
}
