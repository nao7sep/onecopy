// Physical path-safety regressions use only throwaway directories.

use onecopy_lib::path_identity::{directory_is_within, directory_is_within_any};

#[cfg(unix)]
#[test]
fn source_and_destination_symlink_aliases_cannot_bypass_containment() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let child = source.join("nested");
    let source_alias = dir.path().join("source-alias");
    let child_alias = dir.path().join("child-alias");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&source, &source_alias).unwrap();
    std::os::unix::fs::symlink(&child, &child_alias).unwrap();

    assert!(directory_is_within(&child_alias, &source).unwrap());
    assert!(directory_is_within(&child, &source_alias).unwrap());
    assert!(!directory_is_within(&outside, &source_alias).unwrap());
}

#[test]
fn the_root_itself_is_inside_itself_but_a_sibling_is_not() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let sibling = dir.path().join("source-other");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(&sibling).unwrap();

    assert!(directory_is_within(&source, &source).unwrap());
    assert!(!directory_is_within(&sibling, &source).unwrap());
}

#[test]
fn destination_may_be_any_configured_root_or_descendant() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    let child = second.join("child");
    let outside = temp.path().join("outside");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&child).unwrap();
    std::fs::create_dir_all(&outside).unwrap();

    let roots = [first.as_path(), second.as_path()];
    assert!(directory_is_within_any(&child, &roots).unwrap());
    assert!(!directory_is_within_any(&outside, &roots).unwrap());
    assert!(!directory_is_within_any(&outside, &[]).unwrap());
}
