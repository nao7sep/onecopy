// Model-free core integration journeys: the app's public Rust API is exercised
// against a generated corpus on every `cargo test`. These cover wiring between
// subsystems (scan → group → cull → verified move-out; cancel →
// resume). The separate running-app acceptance suite covers the Tauri/webview
// boundary and real AI-backed orchestration.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use chrono_tz::Tz;
use onecopy_lib::operations::{delete_item, move_out, DeleteMode, ItemRef, MoveOutMode};
use onecopy_lib::preview::CachePaths;
use onecopy_lib::{
    derived_state, derived_work, index_store, preview, queries, scanner, similarity, trash,
};
use rusqlite::Connection;

fn item_projection() -> queries::ItemProjectionContext {
    queries::ItemProjectionContext {
        capabilities: derived_state::WorkCapabilities {
            ffmpeg: true,
            video_snapshots_enabled: true,
            similarity_enabled: true,
            face_enabled: false,
            face_models: false,
            transcription_model: false,
            video_transcription_enabled: true,
            audio_transcription_enabled: true,
        },
    }
}

fn january_images(conn: &Connection) -> Vec<queries::SectionItem> {
    queries::section_window(
        conn,
        "image",
        "2026-01",
        Tz::UTC,
        queries::SectionSort {
            order: queries::SectionSortOrder::Time,
            desc: false,
        },
        0,
        queries::MAX_SECTION_WINDOW_ITEMS,
        item_projection(),
    )
    .unwrap()
    .items
}

/// A deterministic JPEG "photo": a gradient with a per-shot brightness lift,
/// so three shots of one scene hash differently but phash identically.
fn shoot(dir: &Path, name: &str, lift: u8, stripes: bool) -> PathBuf {
    let img = image::RgbImage::from_fn(320, 240, |x, y| {
        if stripes {
            if (x / 20) % 2 == 0 {
                image::Rgb([230, 40, 40])
            } else {
                image::Rgb([40, 40, 230])
            }
        } else {
            image::Rgb([
                ((x * 255 / 320) as u8).saturating_add(lift),
                ((y * 255 / 240) as u8).saturating_add(lift),
                90u8.saturating_add(lift),
            ])
        }
    });
    let path = dir.join(name);
    img.save(&path).unwrap();
    path
}

struct World {
    _dir: tempfile::TempDir,
    home: PathBuf,
    corpus: PathBuf,
}

/// One scene of three shots seconds apart (the filename is the timestamp
/// evidence), a duplicate copy of the first shot in a subdirectory, and one
/// unrelated photo an hour later.
fn world(label: &str) -> World {
    let dir = tempfile::Builder::new()
        .prefix(&format!("onecopy-e2e-{label}-"))
        .tempdir()
        .unwrap();
    let home = dir.path().join("home");
    let corpus = dir.path().join("corpus");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(corpus.join("sub")).unwrap();

    shoot(&corpus, "IMG_20260110_120000.jpg", 0, false);
    shoot(&corpus, "IMG_20260110_120010.jpg", 4, false);
    shoot(&corpus, "IMG_20260110_120020.jpg", 8, false);
    std::fs::copy(
        corpus.join("IMG_20260110_120000.jpg"),
        corpus.join("sub/IMG_20260110_120000.jpg"),
    )
    .unwrap();
    shoot(&corpus, "IMG_20260110_130000.jpg", 0, true);
    World {
        _dir: dir,
        home,
        corpus,
    }
}

/// A fixed "now" after the corpus's shooting dates — resolution's
/// plausibility gate is (good-range start … now + a day), so this must sit
/// beyond 2026-01 and be the SAME for every run a test compares.
const NOW_MS: i64 = 1_800_000_000_000;

fn settings(world: &World) -> scanner::ScanSettings {
    let config = serde_json::json!({
        "sourceDirs": [world.corpus.to_string_lossy()],
        "defaultTimezone": "UTC",
    });
    scanner::settings_from_config(Some(&config), &world.home, NOW_MS)
}

fn scan(conn: &Connection, world: &World) -> Result<scanner::ScanSummary, String> {
    scanner::run_full_scan(conn, &settings(world), &|_| {})
}

fn derive_all(conn: &Connection, world: &World) {
    let config = serde_json::json!({});
    let settings = derived_work::settings_from_config(Some(&config), &world.home);
    let cache = CachePaths::new(settings.cache_root.clone());
    let stats = preview::derive_images_pending(
        conn,
        &cache,
        settings.thumb_edge,
        settings.preview_long_edge,
        settings.ffmpeg.as_deref(),
        None,
    )
    .unwrap();
    assert_eq!(stats.derived, 4);
    similarity::rebuild_groups_for_root(conn, &settings.similarity, &world.home).unwrap();
}

fn live_files(corpus: &Path) -> Vec<String> {
    let mut names: Vec<String> = walkdir(corpus)
        .into_iter()
        .map(|p| {
            p.strip_prefix(corpus)
                .unwrap()
                .to_string_lossy()
                .to_string()
        })
        .collect();
    names.sort();
    names
}

fn walkdir(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(walkdir(&path));
        } else {
            files.push(path);
        }
    }
    files
}

#[test]
#[serial_test::serial(scan_cancel)]
fn the_whole_promise_scan_group_cull_and_verified_move_out_cohere() {
    scanner::SCAN_CANCEL.store(false, Ordering::Relaxed);
    let w = world("journey");
    let conn = index_store::open(&w.home.join("index.sqlite3")).unwrap();
    let cache = CachePaths::new(w.home.join("cache"));

    // ---- Index: walk → hash → extract → resolve → pair, then stop ----
    let phases = std::sync::Mutex::new(Vec::<scanner::ScanPhase>::new());
    let summary = scanner::run_full_scan(&conn, &settings(&w), &|progress| {
        let phase = progress.phase;
        let mut phases = phases.lock().unwrap();
        if phases.last().is_none_or(|last| *last != phase) {
            phases.push(phase);
        }
    })
    .expect("the scan completes");
    assert_eq!(summary.seen, 5, "five physical files walked");
    assert_eq!(
        phases.into_inner().unwrap(),
        [
            scanner::ScanPhase::Walk,
            scanner::ScanPhase::Hash,
            scanner::ScanPhase::Extract,
            scanner::ScanPhase::Resolve,
            scanner::ScanPhase::Pair,
            scanner::ScanPhase::Indexed,
        ],
        "scan progress exposes index phases only",
    );
    let underived: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM contents WHERE derived_at_utc IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(underived, 4, "index completion never hides derived work");

    // ---- Derived owner: previews and similarity complete independently ----
    derive_all(&conn, &w);

    // Four logical contents (the duplicate shares its hash), all derived.
    let hashes: Vec<String> = {
        let mut stmt = conn.prepare("SELECT hash FROM contents ORDER BY hash").unwrap();
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        rows
    };
    assert_eq!(hashes.len(), 4);
    for hash in &hashes {
        assert!(cache.thumb(hash).is_file(), "every content has a thumb");
        assert!(cache.preview(hash).is_file(), "every content has a preview");
    }

    // The section a user would open: one month, four logical items, the
    // duplicated shot reporting both copies.
    let items = january_images(&conn);
    assert_eq!(items.len(), 4);
    assert_eq!(
        items.iter().map(|i| i.copy_count).max(),
        Some(2),
        "the duplicated shot counts both copies"
    );

    // The scene grouped; the stripes did not.
    let grouped: i64 = conn
        .query_row("SELECT COUNT(*) FROM similar_group_members", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(grouped, 3, "the three shots of one scene, nothing else");

    // ---- Cull: best-first, losers to trash, winner moved out verified ----
    let a_member = items.iter().find(|i| i.similar_group_id.is_some()).unwrap();
    let members =
        queries::similar_group_of(&conn, a_member.hash.as_ref().unwrap(), true).unwrap();
    assert_eq!(members.len(), 3, "the comparison surface sees the whole scene");
    let winner = members[0].hash.clone();
    let winner_bytes_expected = members[0].byte_size.unwrap() as u64;

    // The corpus's total bytes before the cull: whatever is not exported and
    // not the survivor must land in trash, byte for byte.
    let pre_cull_bytes: u64 = walkdir(&w.corpus)
        .iter()
        .map(|p| std::fs::metadata(p).unwrap().len())
        .sum();

    for loser in &members[1..] {
        let outcome = delete_item(
            &conn,
            &w.home,
            &cache,
            ItemRef::Hash(&loser.hash),
            DeleteMode::Trash,
        )
        .expect("losers trash cleanly");
        assert_eq!(outcome.failed_files, 0);
    }
    let dest = w.home.join("keepers");
    std::fs::create_dir_all(&dest).unwrap();
    let outcome = move_out(
        &conn,
        &w.home,
        &cache,
        ItemRef::Hash(&winner),
        &dest,
        MoveOutMode::MoveTrashRest,
    )
    .expect("the winner moves out");
    assert_eq!(outcome.exported, 1);
    assert!(outcome.conflicts.is_empty() && outcome.undelivered.is_empty());

    // ---- The end state coheres EVERYWHERE at once ----
    // Destination: the winner's exact bytes, verified by re-hashing.
    let exported = walkdir(&dest);
    assert_eq!(exported.len(), 1);
    let exported_bytes = std::fs::read(&exported[0]).unwrap();
    assert_eq!(exported_bytes.len() as u64, winner_bytes_expected);
    assert_eq!(
        blake3::hash(&exported_bytes).to_hex().to_string(),
        winner,
        "the destination holds the winner's exact content"
    );

    // Source tree: only the unrelated photo survives.
    assert_eq!(live_files(&w.corpus), vec!["IMG_20260110_130000.jpg"]);

    // Trash: EVERY corpus byte the cull removed is recoverable — the
    // verified move-out delivers a COPY and routes its sources through trash
    // like any delete, so even the exported original stays restorable. The
    // corpus lives on the home volume, so everything routed into the
    // app-root trash; the overview's own total additionally counts the
    // manifest, honestly reporting what the trash holds on disk.
    let survivor_bytes = std::fs::metadata(w.corpus.join("IMG_20260110_130000.jpg"))
        .unwrap()
        .len();
    let overview =
        trash::overview(&[w.corpus.to_string_lossy().to_string()], &w.home);
    // Scope byte/provenance equality to THIS fixture's trash. The overview
    // deliberately discovers trash on every mounted volume, so summing all of
    // it makes this test depend on unrelated trash left by an earlier run or a
    // real user session on the same machine.
    let trash_files = walkdir(&w.home.join("trash"));
    let media_bytes: u64 = trash_files
        .iter()
        .filter(|p| p.file_name().is_none_or(|n| n != "manifest.jsonl"))
        .map(|p| std::fs::metadata(p).unwrap().len())
        .sum();
    assert_eq!(
        media_bytes,
        pre_cull_bytes - survivor_bytes,
        "every removed corpus byte is in trash"
    );
    let overview_bytes: u64 = overview.iter().map(|r| r.bytes).sum();
    assert!(overview_bytes >= media_bytes, "the surface never under-reports");
    let manifest_lines: usize = trash_files
        .iter()
        .filter(|p| p.file_name().is_some_and(|n| n == "manifest.jsonl"))
        .map(|p| std::fs::read_to_string(p).unwrap().lines().count())
        .sum();
    assert_eq!(manifest_lines, 4, "every trashed file has provenance");

    // DB: the scene is gone as a unit — contents, paths, membership — and the
    // survivor is intact.
    let (contents, live_paths, memberships): (i64, i64, i64) = (
        conn.query_row("SELECT COUNT(*) FROM contents", [], |r| r.get(0))
            .unwrap(),
        conn.query_row("SELECT COUNT(*) FROM paths WHERE missing = 0", [], |r| {
            r.get(0)
        })
        .unwrap(),
        conn.query_row("SELECT COUNT(*) FROM similar_group_members", [], |r| {
            r.get(0)
        })
        .unwrap(),
    );
    assert_eq!((contents, live_paths, memberships), (1, 1, 0));

    // Cache: culled contents swept, the survivor still served.
    for hash in &hashes {
        let expect = conn
            .query_row(
                "SELECT COUNT(*) FROM contents WHERE hash = ?1",
                [hash],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
            == 1;
        assert_eq!(cache.thumb(hash).is_file(), expect, "thumb presence follows the DB");
        assert_eq!(cache.preview(hash).is_file(), expect, "preview presence follows the DB");
    }

    // The month view and the issues surface agree nothing is wrong.
    let items = january_images(&conn);
    assert_eq!(items.len(), 1);
    let (issue_total, _) = queries::issues(&conn, 10).unwrap();
    assert_eq!(issue_total, 0, "a clean journey raises no issues");
}

#[test]
#[serial_test::serial(scan_cancel)]
fn a_cancelled_index_resumes_to_the_same_facts_an_uninterrupted_run_builds() {
    let interrupted = world("resume");
    let control = World {
        home: interrupted._dir.path().join("control-home"),
        corpus: interrupted.corpus.clone(),
        _dir: tempfile::Builder::new().prefix("onecopy-e2e-unused-").tempdir().unwrap(),
    };
    std::fs::create_dir_all(&control.home).unwrap();

    // ---- Interrupt between hash and metadata extraction ----
    scanner::SCAN_CANCEL.store(false, Ordering::Relaxed);
    let conn = index_store::open(&interrupted.home.join("index.sqlite3")).unwrap();
    let interrupted_settings = settings(&interrupted);
    let err = scanner::run_full_scan(&conn, &interrupted_settings, &|progress| {
        let phase = progress.phase;
        // Walk and hash are done; extraction's per-row check takes the hit,
        // leaving real work on BOTH sides of the cut.
        if phase == scanner::ScanPhase::Hash && progress.done == progress.total {
            scanner::SCAN_CANCEL.store(true, Ordering::Relaxed);
        }
    })
    .expect_err("the cancelled scan must not report success");
    assert_eq!(err, scanner::CANCELLED);

    // ---- Resume finishes; the control never stopped ----
    scanner::SCAN_CANCEL.store(false, Ordering::Relaxed);
    scanner::run_full_scan(&conn, &interrupted_settings, &|_| {})
        .expect("the resume completes");
    let control_conn = index_store::open(&control.home.join("index.sqlite3")).unwrap();
    scan(&control_conn, &control).expect("the control scan completes");

    // ---- Identical index content, row for row ----
    let dump = |conn: &Connection| -> (Vec<String>, Vec<String>) {
        let mut contents = Vec::new();
        let mut stmt = conn
            .prepare(
                "SELECT hash, byte_size, kind \
                 FROM contents ORDER BY hash",
            )
            .unwrap();
        let mut rows = stmt.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            contents.push(format!(
                "{}|{}|{}",
                row.get::<_, String>(0).unwrap(),
                row.get::<_, i64>(1).unwrap(),
                row.get::<_, String>(2).unwrap(),
            ));
        }
        let mut paths = Vec::new();
        let mut stmt = conn
            .prepare(
                "SELECT abs_path, content_hash, resolved_utc_ms, resolved_source, missing \
                 FROM paths ORDER BY abs_path",
            )
            .unwrap();
        let mut rows = stmt.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            paths.push(format!(
                "{}|{:?}|{:?}|{:?}|{}",
                row.get::<_, String>(0).unwrap(),
                row.get::<_, Option<String>>(1).unwrap(),
                row.get::<_, Option<i64>>(2).unwrap(),
                row.get::<_, Option<String>>(3).unwrap(),
                row.get::<_, i64>(4).unwrap(),
            ));
        }
        (contents, paths)
    };
    let (resumed_contents, resumed_paths) = dump(&conn);
    let (control_contents, control_paths) = dump(&control_conn);
    assert_eq!(resumed_contents, control_contents, "contents identical after resume");
    assert_eq!(resumed_paths, control_paths, "paths identical after resume");

    let derived = |conn: &Connection| -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM contents WHERE derived_at_utc IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap()
    };
    assert_eq!((derived(&conn), derived(&control_conn)), (0, 0));
}
