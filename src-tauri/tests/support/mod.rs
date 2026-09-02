use std::path::{Path, PathBuf};

use onecopy_lib::binaries::BinaryStatus;
use onecopy_lib::binaries_manager::{install_entry, installed_path, spec_of, state_of};

pub fn company_fixtures() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../company/assets/test-fixtures");
    assert!(
        root.join("manifest.json").is_file(),
        "company fixture checkout is required at {}",
        root.display()
    );
    root
}

pub fn managed_root() -> PathBuf {
    let root = std::env::var_os("ONECOPY_TEST_MANAGED_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target/ai-acceptance-home"));
    std::fs::create_dir_all(&root).expect("create acceptance managed root");
    root
}

pub fn ensure_managed(id: &str) -> PathBuf {
    let root = managed_root();
    let spec = spec_of(id).unwrap_or_else(|| panic!("managed dependency is registered: {id}"));
    let state = state_of(&root, spec);
    if state.status != BinaryStatus::UpToDate {
        eprintln!("installing managed dependency {id} into {}", root.display());
        install_entry(&root, id, |progress| eprintln!("{id}: {progress:?}"))
            .unwrap_or_else(|error| panic!("install {id}: {error}"));
    }
    let path = installed_path(&root, spec);
    assert!(path.is_file(), "{id} artifact exists at {}", path.display());
    eprintln!(
        "{id}: {} ({} bytes)",
        path.display(),
        std::fs::metadata(&path).unwrap().len()
    );
    path
}
