use super::*;

#[test]
fn libraries_and_apps_are_packages_in_any_case() {
    for name in ["Photos Library.photoslibrary", "Catalog Previews.lrdata", "Film.fcpbundle", "OneCopy.app", "LIB.PhotosLibrary"] {
        assert!(is_package_name(name.as_ref()), "{name}");
    }
    for name in ["Photos", "trip.2024", "photoslibrary", "notes.txt"] {
        assert!(!is_package_name(name.as_ref()), "{name}");
    }
}

#[test]
fn anything_below_a_package_is_within_it() {
    assert!(within_package(Path::new("/Pictures/Photos Library.photoslibrary/originals/A/IMG_1.HEIC"), false));
    assert!(within_package(Path::new("/Pictures/Photos Library.photoslibrary"), true));
    assert!(within_package(Path::new("/Pictures/Photos Library.photoslibrary/originals"), true));
    assert!(!within_package(Path::new("/Pictures/Trip/IMG_1.HEIC"), false));
    // A regular file with a package extension is a file, not a folder.
    assert!(!within_package(Path::new("/Downloads/Installer.app"), false));
}
