// A test whose subject exists only on Windows (junctions, file attributes,
// drive letters, job objects). Elsewhere it is listed as ignored with its
// reason rather than compiled out, so a run on the Mac shows what it did not
// cover; forced to run there (`--include-ignored`), it fails rather than
// passing with nothing tested. On Windows it is an ordinary test, so the
// default suite runs it. The reason is a literal starting "Windows only: ",
// because libtest prints only a literal `ignore` reason.
//
// Included by every integration suite and, under cfg(test), by the library
// before the modules whose unit tests use it.
macro_rules! windows_test {
    ($reason:literal, $(#[$meta:meta])* fn $name:ident() $body:block) => {
        $(#[$meta])*
        #[test]
        #[cfg_attr(not(windows), ignore = $reason)]
        fn $name() {
            #[cfg(windows)]
            $body
            #[cfg(not(windows))]
            panic!("{}; run it on Windows", $reason);
        }
    };
}
