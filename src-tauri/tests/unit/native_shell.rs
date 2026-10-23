use super::*;
use std::os::unix::ffi::OsStrExt;

#[test]
fn file_url_retains_native_bytes_without_opening_finder() {
    let path = Path::new(std::ffi::OsStr::from_bytes(b"/tmp/native-\xff.jpg"));
    let url = file_url(path, false).unwrap();
    let bytes = unsafe { std::ffi::CStr::from_ptr(url.fileSystemRepresentation().as_ptr()) };
    assert_eq!(bytes.to_bytes(), path.as_os_str().as_bytes());
    assert_eq!(
        crate::native_path::from_text(&encoded_url_path(&url)).as_ref(),
        path
    );
}
