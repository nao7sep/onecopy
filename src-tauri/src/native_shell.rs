//! Native operands for shell integration; never reopen a display spelling.

use std::path::Path;

#[cfg(target_os = "macos")]
pub(crate) async fn pick_directories(
    window: tauri::Window,
    multiple: bool,
) -> Result<Vec<String>, String> {
    use objc2_app_kit::{NSModalResponseCancel, NSModalResponseOK, NSOpenPanel, NSWindow};
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let owner = window.clone();
    window
        .run_on_main_thread(move || {
            let parent = match owner.ns_window() {
                Ok(parent) => parent,
                Err(error) => {
                    let _ = sender.send(Err(error.to_string()));
                    return;
                }
            };
            let panel = NSOpenPanel::openPanel(
                objc2::MainThreadMarker::new().expect("main-thread dispatch"),
            );
            panel.setCanCreateDirectories(true);
            panel.setCanChooseFiles(false);
            panel.setCanChooseDirectories(true);
            panel.setAllowsMultipleSelection(multiple);
            let selected = panel.clone();
            let sender = std::cell::RefCell::new(Some(sender));
            let completion = block2::RcBlock::new(move |response| {
                let result = if response == NSModalResponseOK {
                    Ok(selected
                        .URLs()
                        .iter()
                        .map(|url| encoded_url_path(&url))
                        .collect())
                } else if response == NSModalResponseCancel {
                    Ok(Vec::new())
                } else {
                    Err("folder picker could not be displayed".to_string())
                };
                if let Some(sender) = sender.borrow_mut().take() {
                    let _ = sender.send(result);
                }
            });
            // Tauri supplies this window's live AppKit object; the sheet and its
            // copied completion block own the picker until the user settles it.
            panel.beginSheetModalForWindow_completionHandler(
                unsafe { &*parent.cast::<NSWindow>() },
                &completion,
            );
        })
        .map_err(|error| error.to_string())?;
    receiver
        .await
        .map_err(|_| "folder picker ended without a result".to_string())?
}

#[cfg(target_os = "macos")]
fn encoded_url_path(url: &objc2_foundation::NSURL) -> String {
    use std::os::unix::ffi::OsStrExt;
    // NSURL owns its terminated filesystem representation for this call.
    let bytes = unsafe { std::ffi::CStr::from_ptr(url.fileSystemRepresentation().as_ptr()) };
    crate::native_path::encode(std::ffi::OsStr::from_bytes(bytes.to_bytes())).into_owned()
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn reveal(path: &Path) -> Result<(), String> {
    tauri_plugin_opener::reveal_item_in_dir(path).map_err(|error| error.to_string())
}

#[cfg(target_os = "macos")]
pub(crate) fn reveal(path: &Path) -> Result<(), String> {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::NSArray;
    let path = crate::volume_io::canonicalize(path).map_err(|error| error.to_string())?;
    let directory = crate::volume_io::is_dir(&path).map_err(|error| error.to_string())?;
    let url = file_url(&path, directory)?;
    NSWorkspace::new().activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[url]));
    Ok(())
}

// The plugin's macOS reveal adapter constructs NSString from lossy UTF-8.
// Foundation's filesystem initializer preserves the native byte representation.
#[cfg(target_os = "macos")]
fn file_url(
    path: &Path,
    directory: bool,
) -> Result<objc2::rc::Retained<objc2_foundation::NSURL>, String> {
    use std::os::unix::ffi::OsStrExt;
    let bytes =
        std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|error| error.to_string())?;
    // CString keeps a terminated, non-null pointer alive for the initializer.
    Ok(unsafe {
        objc2_foundation::NSURL::fileURLWithFileSystemRepresentation_isDirectory_relativeToURL(
            std::ptr::NonNull::new(bytes.as_ptr().cast_mut()).unwrap(),
            directory,
            None,
        )
    })
}

#[cfg(all(test, target_os = "macos"))]
#[path = "../tests/unit/native_shell.rs"]
mod tests;

#[cfg(not(target_os = "macos"))]
pub(crate) async fn pick_directories(
    window: tauri::Window,
    multiple: bool,
) -> Result<Vec<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let builder = window.dialog().file();
    #[cfg(windows)]
    let builder = builder.set_parent(&window);
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let settled = move |paths: Option<Vec<tauri_plugin_dialog::FilePath>>| {
        let result = paths
            .unwrap_or_default()
            .into_iter()
            .map(|path| {
                path.into_path()
                    .map(|path| crate::native_path::encode(&path).into_owned())
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, _>>();
        let _ = sender.send(result);
    };
    if multiple {
        builder.pick_folders(settled);
    } else {
        builder.pick_folder(move |path| settled(path.map(|path| vec![path])));
    }
    receiver
        .await
        .map_err(|_| "folder picker ended without a result".to_string())?
}
