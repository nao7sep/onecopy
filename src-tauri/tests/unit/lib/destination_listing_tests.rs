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

    let rows = list_subdirs_at(root.path(), &visibility::Policy::from_config(&json!({})).unwrap()).unwrap();
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
    assert!(list_subdirs_at(&root.path().join("trash-only/.onecopy-trash"), &visibility::Policy::from_config(&json!({})).unwrap())
        .unwrap().is_empty());
}
