use super::*;
#[path = "windows_security_fixture.rs"]
mod security_fixture;
use security_fixture::{create_directory_with_security, security_text, directory_security_text};

fn shared_root(parent: &Path, name: &str) -> PathBuf {
    let path = parent.join(name);
    let key = security::account_key().unwrap();
    create_directory_with_security(&path, &format!("O:{key}D:P(A;OICI;FA;;;{key})(A;OICI;FR;;;WD)"));
    path
}

#[test]
fn new_root_is_private_from_creation_and_certificate_is_bound_to_its_identity() {
    let parent = tempfile::tempdir().unwrap();
    let broad = shared_root(parent.path(), "shared");
    let root = broad.join("app");
    assert!(security::create_directory(&root).unwrap());
    let handle = security::pin_owned(&root, true).unwrap();
    security::require_private(&handle, true).unwrap();
    write_certificate(&root, &handle).unwrap();
    verify(&root).unwrap();
    let copy = broad.join("copy");
    security::create_directory(&copy).unwrap();
    let mut marker = security::create_file(&copy.join(CERTIFICATE)).unwrap();
    marker.write_all(&std::fs::read(root.join(CERTIFICATE)).unwrap()).unwrap();
    drop(marker);
    assert!(verify(&copy).is_err());
}

#[test]
fn consent_protects_owned_descendants_without_changing_unrelated_objects() {
    let parent = tempfile::tempdir().unwrap();
    let root = shared_root(parent.path(), "app");
    std::fs::create_dir(root.join("cache")).unwrap();
    std::fs::write(root.join("cache/preview.bin"), b"derived bytes").unwrap();
    std::fs::write(root.join("config.json"), b"saved settings").unwrap();
    std::fs::create_dir(root.join("unrelated")).unwrap();
    let unknown = root.join("unrelated/document.txt");
    std::fs::write(&unknown, b"original document").unwrap();
    let before = security_text(&File::open(&unknown).unwrap());
    let before_directory = directory_security_text(&root.join("unrelated"));
    assert!(verify(&root).is_err());
    protect(&root).unwrap();
    verify(&root).unwrap();
    for path in [root.join("cache/preview.bin"), root.join("config.json")] {
        let file = security::pin_owned(&path, false).unwrap();
        security::require_private(&file, false).unwrap();
    }
    assert_eq!(security_text(&File::open(&unknown).unwrap()), before);
    assert_eq!(directory_security_text(&root.join("unrelated")), before_directory);
    assert_eq!(std::fs::read(&unknown).unwrap(), b"original document");
    assert_eq!(std::fs::read(root.join("config.json")).unwrap(), b"saved settings");
}

#[test]
fn failed_migration_invalidates_previous_certificate_and_never_changes_external_hardlinks() {
    let parent = tempfile::tempdir().unwrap();
    let root = shared_root(parent.path(), "app");
    protect(&root).unwrap();
    let external = parent.path().join("original.bin");
    std::fs::write(&external, b"original bytes").unwrap();
    std::fs::create_dir(root.join("cache")).unwrap();
    let link = root.join("cache/linked.bin");
    std::fs::hard_link(&external, &link).unwrap();
    let before = security_text(&File::open(&external).unwrap());
    assert!(protect(&root).is_err());
    assert!(verify(&root).is_err());
    assert_eq!(security_text(&File::open(&external).unwrap()), before);
    assert_eq!(std::fs::read(&external).unwrap(), b"original bytes");
    std::fs::remove_file(link).unwrap();
    protect(&root).unwrap();
    verify(&root).unwrap();
}

#[test]
fn privacy_recovery_refuses_junctions_and_preserves_their_targets() {
    let parent = tempfile::tempdir().unwrap();
    let root = shared_root(parent.path(), "app");
    let external = shared_root(parent.path(), "external");
    std::fs::write(external.join("original.txt"), b"untouched").unwrap();
    let before = directory_security_text(&external);
    let junction = root.join("cache");
    let output = std::process::Command::new("cmd").args(["/C", "mklink", "/J"])
        .arg(&junction).arg(&external).output().unwrap();
    assert!(output.status.success(), "junction fixture: {}", String::from_utf8_lossy(&output.stderr));
    assert!(protect(&root).is_err());
    assert!(verify(&root).is_err());
    assert_eq!(directory_security_text(&external), before);
    assert_eq!(std::fs::read(external.join("original.txt")).unwrap(), b"untouched");
    std::fs::remove_dir(junction).unwrap();
}

#[test]
fn held_root_rejects_replacement_in_a_shared_parent() {
    let parent = tempfile::tempdir().unwrap();
    let root = shared_root(parent.path(), "app");
    let handle = security::pin_owned(&root, true).unwrap();
    assert!(std::fs::rename(&root, parent.path().join("moved")).is_err());
    drop(handle);
    std::fs::rename(&root, parent.path().join("moved")).unwrap();
}
