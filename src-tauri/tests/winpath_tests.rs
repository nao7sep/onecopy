// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).
//
// The Windows long-path grammar, asserted on EVERY host. The rules are fiddly
// and the consequence of getting them wrong is silent — a photo beyond the
// classic limit simply never enters the app — so leaving them provable only on
// the machine we visit least would be the worst possible arrangement.

use onecopy_lib::winpath::{extended_form, for_display};

#[test]
fn a_drive_absolute_path_gets_the_verbatim_prefix() {
    assert_eq!(
        extended_form(r"C:\photos\2016\IMG_0001.jpg").as_deref(),
        Some(r"\\?\C:\photos\2016\IMG_0001.jpg")
    );
    // Any drive letter, either case.
    assert_eq!(extended_form(r"d:\x").as_deref(), Some(r"\\?\d:\x"));
}

#[test]
fn forward_slashes_become_backslashes_first() {
    // A verbatim path takes only backslashes; a forward slash would stay a
    // literal character inside the file name instead of separating it.
    assert_eq!(
        extended_form("C:/photos/a.jpg").as_deref(),
        Some(r"\\?\C:\photos\a.jpg")
    );
}

#[test]
fn a_network_share_takes_the_unc_form() {
    assert_eq!(
        extended_form(r"\\nas\photos\a.jpg").as_deref(),
        Some(r"\\?\UNC\nas\photos\a.jpg")
    );
    // A server with no share is not a usable root.
    assert_eq!(extended_form(r"\\nas"), None);
}

#[test]
fn already_verbatim_paths_are_left_alone() {
    // Prefixing twice yields a path that resolves to nothing.
    assert_eq!(extended_form(r"\\?\C:\photos\a.jpg"), None);
    assert_eq!(extended_form(r"\\?\UNC\nas\photos\a.jpg"), None);
}

#[test]
fn device_paths_are_left_alone() {
    assert_eq!(extended_form(r"\\.\PhysicalDrive0"), None);
}

#[test]
fn relative_paths_are_left_alone() {
    assert_eq!(extended_form(r"photos\a.jpg"), None);
    assert_eq!(extended_form(r"\photos\a.jpg"), None);
    // Drive-RELATIVE despite the drive letter: C:folder means "folder, on C's
    // current directory", which a verbatim prefix would silently change.
    assert_eq!(extended_form(r"C:photos"), None);
}

#[test]
fn a_parent_component_is_refused_rather_than_guessed() {
    // Verbatim paths are handed to the filesystem WITHOUT normalization, so
    // `..` would stop meaning "parent" and the path would address something
    // else entirely. Refusing keeps the classic path and the classic limit,
    // which is wrong-but-visible rather than wrong-and-silent.
    assert_eq!(extended_form(r"C:\photos\..\other\a.jpg"), None);
    assert_eq!(extended_form("C:/photos/../a.jpg"), None);
    // A file merely CONTAINING dots is fine.
    assert_eq!(
        extended_form(r"C:\photos\my..album\a.jpg").as_deref(),
        Some(r"\\?\C:\photos\my..album\a.jpg")
    );
}

#[test]
fn display_strips_the_prefix_back_off() {
    // The user reads these in the metadata pane's copy list and the issues
    // list; \\?\C:\photos\a.jpg is not what anyone recognises as a location.
    assert_eq!(for_display(r"\\?\C:\photos\a.jpg"), r"C:\photos\a.jpg");
    assert_eq!(for_display(r"\\?\UNC\nas\photos\a.jpg"), r"\\nas\photos\a.jpg");
    // Untouched when there is nothing to strip.
    assert_eq!(for_display(r"C:\photos\a.jpg"), r"C:\photos\a.jpg");
    assert_eq!(for_display("/Users/x/photos/a.jpg"), "/Users/x/photos/a.jpg");
}

#[test]
fn the_transform_round_trips_through_display() {
    for original in [
        r"C:\photos\2016\spain\beach.jpg",
        r"\\nas\media\clip.mov",
    ] {
        let extended = extended_form(original).expect("absolute paths convert");
        assert_eq!(for_display(&extended), original, "{original}");
    }
}

#[test]
fn a_path_past_the_classic_limit_is_exactly_what_this_is_for() {
    // 260 is the classic cap. A real backup tree reaches it with ordinary
    // folder names, and today such a file is simply invisible to the app.
    let deep = format!(r"C:\{}\IMG_0001.jpg", vec!["a-folder-name"; 20].join("\\"));
    assert!(deep.len() > 260, "the fixture must actually exceed the limit");
    let extended = extended_form(&deep).expect("it converts");
    assert!(extended.starts_with(r"\\?\"));
    assert_eq!(for_display(&extended), deep);
}
