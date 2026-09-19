//! What the heavy suite's subjects need to run for real: OneCopy's managed
//! tools and models, and the shared test-fixture corpus. Tests import their
//! subjects directly; this module only supplies the environment.
//!
//! The artifacts are acquired through the production Managed tools path into a
//! cache that persists between runs, following the app's own rule: install what
//! is missing, update ffmpeg only when its upstream check finds a newer build,
//! and replace a model only when this build pins a different one. Each test
//! scans copies of corpus files into its own disposable app home, where the
//! cached artifacts are hard-linked, so no test writes to the cache or corpus.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use onecopy_lib::binaries::BinaryStatus;
use onecopy_lib::binaries_manager::{self, BIN_DIR_NAME, DEPENDENCIES, MODELS_DIR_NAME};
use onecopy_lib::preview::CachePaths;
use onecopy_lib::{derived_work, index_store, scanner};
use rusqlite::Connection;

const NOW_MS: i64 = 1_800_000_000_000;

pub struct ArtifactCache {
    pub root: PathBuf,
    /// Why ffmpeg could not be compared with upstream, when its check failed.
    /// The cached build still serves every other heavy test.
    pub ffmpeg_check: Result<(), String>,
}

pub fn artifacts() -> &'static ArtifactCache {
    static CACHE: OnceLock<Result<ArtifactCache, String>> = OnceLock::new();
    match CACHE.get_or_init(prepare_artifacts) {
        Ok(cache) => cache,
        Err(error) => panic!("the managed tools and models could not be prepared: {error}"),
    }
}

fn prepare_artifacts() -> Result<ArtifactCache, String> {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("managed-artifacts");
    std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    binaries_manager::reset_temp_dir(&root);
    let mut ffmpeg_check = Ok(());
    let platform_specs = DEPENDENCIES
        .iter()
        .filter(|spec| binaries_manager::spec_of(spec.id).is_some());
    for spec in platform_specs {
        let status = || binaries_manager::state_of(&root, spec).status;
        let install = if status() == BinaryStatus::NotInstalled {
            true
        } else if spec.pinned.is_some() {
            status() != BinaryStatus::UpToDate
        } else {
            match binaries_manager::check_entry(&root, spec.id) {
                Ok(_) => status() != BinaryStatus::UpToDate,
                Err(error) => {
                    ffmpeg_check = Err(error);
                    false
                }
            }
        };
        if install {
            binaries_manager::install_entry(&root, spec.id, |_| {})
                .map_err(|error| format!("{}: {error}", spec.id))?;
        }
    }
    Ok(ArtifactCache { root, ffmpeg_check })
}

/// The shared corpus, checked out beside this repository as ~/code/company.
pub fn corpus() -> PathBuf {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../company/assets/test-fixtures");
    assert!(
        corpus.join("manifest.json").is_file(),
        "the heavy suite reads the shared test-fixture corpus at {}; check out the company repository beside this one",
        corpus.display()
    );
    corpus
}

pub fn corpus_file(relative: &str) -> PathBuf {
    corpus().join(relative)
}

pub fn corpus_json(relative: &str) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(corpus_file(relative)).unwrap()).unwrap()
}

/// The manifest's recorded facts, one entry per corpus file.
pub fn manifest() -> Vec<serde_json::Value> {
    corpus_json("manifest.json")["fixtures"]
        .as_array()
        .expect("the manifest lists its fixtures")
        .clone()
}

/// A disposable app home whose managed tools and models are the cached ones.
pub fn app_home(label: &str) -> tempfile::TempDir {
    let cache = artifacts();
    // Inside the target directory, so the hard links stay on one volume.
    let home = tempfile::Builder::new()
        .prefix(&format!("heavy-{label}-"))
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .unwrap();
    for directory in [BIN_DIR_NAME, MODELS_DIR_NAME] {
        link_tree(&cache.root.join(directory), &home.path().join(directory));
    }
    home
}

fn link_tree(from: &Path, to: &Path) {
    if !from.is_dir() {
        return;
    }
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            link_tree(&entry.path(), &target);
        } else {
            std::fs::hard_link(entry.path(), target).unwrap();
        }
    }
}

/// An indexed library inside a disposable app home.
pub struct Library {
    pub home: tempfile::TempDir,
    pub source: PathBuf,
    pub conn: Connection,
    pub cache: CachePaths,
    pub settings: derived_work::Settings,
}

/// Copies `files` into a fresh app home and scans them, configured exactly as
/// the app would be with `config` merged into the settings.
pub fn library(label: &str, files: &[PathBuf], config: serde_json::Value) -> Library {
    let home = app_home(label);
    let source = home.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    for file in files {
        std::fs::copy(file, source.join(file.file_name().unwrap()))
            .unwrap_or_else(|error| panic!("copying {}: {error}", file.display()));
    }
    let mut config_json = serde_json::json!({
        "sourceDirs": [source.to_string_lossy()],
        "defaultTimezone": "UTC",
    });
    for (key, value) in config.as_object().into_iter().flatten() {
        config_json[key] = value.clone();
    }
    let conn = index_store::open(&home.path().join("index.sqlite3")).unwrap();
    let scan = scanner::settings_from_config(Some(&config_json), home.path(), NOW_MS);
    scanner::run_full_scan(&conn, &scan, &|_| {}).unwrap();
    let settings = derived_work::settings_from_config(Some(&config_json), home.path()).unwrap();
    let cache = CachePaths::new(settings.cache_root.clone());
    Library {
        home,
        source,
        conn,
        cache,
        settings,
    }
}

impl Library {
    pub fn ffmpeg(&self) -> &Path {
        self.settings.ffmpeg.as_deref().expect("the cached ffmpeg is linked into every home")
    }

    /// The current content hash of the scanned copy named `file_name`.
    pub fn hash_of(&self, file_name: &str) -> String {
        self.conn
            .query_row(
                "SELECT content_hash FROM paths WHERE file_name = ?1",
                [file_name],
                |row| row.get(0),
            )
            .unwrap_or_else(|error| panic!("{file_name} is not indexed: {error}"))
    }

    pub fn path_of(&self, file_name: &str) -> PathBuf {
        self.source.join(file_name)
    }
}
