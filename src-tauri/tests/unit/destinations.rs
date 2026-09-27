use super::*;

#[test]
fn one_listing_projects_children_and_emptiness_together() {
    let root = tempfile::Builder::new()
        .prefix("onecopy-destinations-")
        .tempdir()
        .unwrap();
    std::fs::create_dir(root.path().join("empty")).unwrap();
    std::fs::create_dir(root.path().join("files-only")).unwrap();
    std::fs::write(root.path().join("files-only/item.txt"), b"item").unwrap();
    std::fs::create_dir_all(root.path().join("nested/child")).unwrap();
    std::fs::create_dir(root.path().join(".hidden")).unwrap();
    std::fs::create_dir_all(root.path().join("hidden-only/.hidden")).unwrap();
    std::fs::create_dir_all(root.path().join("trash-only/.onecopy-trash/day")).unwrap();
    std::fs::create_dir_all(root.path().join("app-data/cache")).unwrap();
    let data_root = root.path().join("app-data");
    let unused_data_root = root.path().join("never-used-app-data");

    let rows = list_subdirs_at(root.path(), &visibility::Policy::from_config(&json!({})).unwrap(), &unused_data_root).unwrap();
    let facts = |name: &str| {
        let row = rows.iter().find(|row| row.name == name).unwrap();
        (row.has_children, row.is_empty)
    };
    assert_eq!(facts("empty"), (false, true));
    assert_eq!(facts("files-only"), (false, false));
    assert_eq!(facts("nested"), (true, false));
    assert_eq!(facts("hidden-only"), (false, false));
    assert_eq!(facts("trash-only"), (false, false));
    assert!(rows.iter().all(|row| row.name != ".hidden"));
    assert!(list_subdirs_at(
        &root.path().join("trash-only/.onecopy-trash"),
        &visibility::Policy::from_config(&json!({})).unwrap(),
        &unused_data_root,
    )
    .unwrap()
    .is_empty());

    // R6-02: the app's own data root is never a browsable destination child,
    // and listing straight into it (a source containing the data root, or a
    // destination path resolving inside it) answers empty like trash.
    let rows_excluding_data_root =
        list_subdirs_at(root.path(), &visibility::Policy::from_config(&json!({})).unwrap(), &data_root).unwrap();
    assert!(rows_excluding_data_root.iter().all(|row| row.name != "app-data"));
    assert!(list_subdirs_at(&data_root, &visibility::Policy::from_config(&json!({})).unwrap(), &data_root)
        .unwrap()
        .is_empty());
}

#[test]
fn folder_names_are_trimmed_and_refused_when_unusable() {
    assert_eq!(folder_name("  Trip  ").unwrap(), "Trip");
    for refused in ["", "   ", "a/b", "a\\b", "line\nbreak", "tab\there"] {
        assert!(folder_name(refused).is_err(), "{refused:?} was accepted");
    }
}

#[test]
fn a_new_folder_must_be_unique_ignoring_case() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("Trip")).unwrap();
    assert!(create_subdir(root.path(), "trip").is_err());
    let created = create_subdir(root.path(), " Other ").unwrap();
    assert_eq!(std::path::PathBuf::from(created), root.path().join("Other"));
    assert!(root.path().join("Other").is_dir());
}

#[test]
fn only_an_empty_folder_is_removed() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("full/child")).unwrap();
    std::fs::create_dir(root.path().join("empty")).unwrap();
    assert!(delete_empty_dir(&root.path().join("full")).is_err());
    delete_empty_dir(&root.path().join("empty")).unwrap();
    assert!(!root.path().join("empty").exists());
}

// The tree never waits on a drive that does not answer: listing it fails
// within the bound, and a child on such a drive shows as expandable and not
// empty (so nothing offers to delete it) while its siblings list normally.
#[test]
fn a_drive_that_does_not_answer_fails_the_listing_within_its_bound() {
    use crate::volume_io::{FakeStallingVolume, Op};
    let root = tempfile::Builder::new()
        .prefix("onecopy-destinations-stall-")
        .tempdir()
        .unwrap();
    std::fs::create_dir(root.path().join("away")).unwrap();
    std::fs::create_dir(root.path().join("here")).unwrap();
    let policy = visibility::Policy::from_config(&json!({})).unwrap();
    let data_root = root.path().join("never-used-app-data");

    let away = FakeStallingVolume::mount(&root.path().join("away"), std::time::Duration::from_millis(200));
    away.stall(&[Op::List], None);
    let started = std::time::Instant::now();
    let rows = list_subdirs_at(root.path(), &policy, &data_root).unwrap();
    let row = |name: &str| rows.iter().find(|row| row.name == name).map(|row| (row.has_children, row.is_empty));
    assert_eq!(row("away"), Some((true, false)));
    assert_eq!(row("here"), Some((false, true)));
    assert!(list_subdirs_at(&root.path().join("away"), &policy, &data_root).is_err());
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
    away.release();
    assert!(away.wait_until_settled(std::time::Duration::from_secs(5)));
}
