//! Windows data-root privacy is established before ordinary stores open.
//! Only the explicit recovery action changes an existing tree's permissions.
use std::{fs::File, io::{self, Read, Seek, SeekFrom, Write}, path::{Path, PathBuf}, sync::{Mutex, OnceLock}};
use serde::{Deserialize, Serialize};
use crate::{paths, windows_account_privacy as security};

use paths::PRIVATE_ROOT_CERTIFICATE as CERTIFICATE;
// Retain the root's no-delete-sharing handle for the process, including while
// startup is blocked. Later work cannot silently switch to a replacement root.
static ROOT: OnceLock<(PathBuf, File)> = OnceLock::new();
static RECOVERY: Mutex<()> = Mutex::new(());

#[derive(Serialize, Deserialize)]
struct Certificate {
    version: u32,
    root: crate::file_identity::FileIdentity,
    account: String,
}

pub(crate) fn prepare(root: &Path) -> io::Result<()> {
    let native = crate::winpath::for_fs(root);
    let root = native.as_ref();
    if let Some((held, _)) = ROOT.get() {
        return if held == root { Ok(()) } else { Err(io::Error::other("data root changed during launch")) };
    }
    if let Some(parent) = root.parent() { std::fs::create_dir_all(parent)?; }
    let created = security::create_directory(root)?;
    let handle = security::pin_owned(root, true)?;
    if created { write_certificate(root, &handle)?; }
    ROOT.set((root.to_path_buf(), handle)).map_err(|_| io::Error::other("data root was prepared twice"))
}

pub(crate) fn verify(root: &Path) -> io::Result<()> {
    let native = crate::winpath::for_fs(root);
    let root = native.as_ref();
    let handle = security::pin_owned(root, true)?;
    security::require_private(&handle, true)?;
    let mut file = security::pin_owned(&root.join(CERTIFICATE), false)?;
    security::require_private(&file, false)?;
    // This record is constant-sized identity evidence, not an input-size buffer.
    let mut bytes = Vec::new();
    (&mut file).take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 { return Err(io::Error::other("privacy certificate is oversized")); }
    let certificate: Certificate = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if certificate.version != 1 || certificate.root != crate::file_identity::FileIdentity::from_file(&handle)?
        || certificate.account != security::account_key()?
    {
        return Err(io::Error::other("data folder privacy has not been established for this account and folder"));
    }
    Ok(())
}

fn certificate_file(root: &Path) -> io::Result<File> {
    let path = root.join(CERTIFICATE);
    match security::pin_owned(&path, false) {
        Ok(file) => Ok(file),
        Err(error) if error.kind() == io::ErrorKind::NotFound => security::create_file(&path),
        Err(error) => Err(error),
    }
}

fn write_certificate(root: &Path, handle: &File) -> io::Result<()> {
    // not recorded: reconstructible permission proof, with no authored settings.
    let certificate = Certificate { version: 1, root: crate::file_identity::FileIdentity::from_file(handle)?, account: security::account_key()? };
    let mut file = certificate_file(root)?;
    // New certificates are already private; existing ones were tightened by
    // migration. A partial record never verifies and requires recovery again.
    security::require_private(&file, false)?;
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    serde_json::to_writer(&mut file, &certificate).map_err(io::Error::other)?;
    file.flush()?;
    file.sync_all()
}

pub(crate) fn protect(root: &Path) -> io::Result<()> {
    let _recovery = RECOVERY.try_lock().map_err(|_| io::Error::other("folder protection is already running"))?;
    let native = crate::winpath::for_fs(root);
    let root = native.as_ref();
    let root_handle = security::pin_owned(root, true)?;
    // Invalidate old evidence durably BEFORE tightening the root. Otherwise a
    // crash midway through a repeated migration could revive an old stamp.
    match security::pin_owned(&root.join(CERTIFICATE), false) {
        Ok(file) => { file.set_len(0)?; file.sync_all()?; },
        Err(error) if error.kind() == io::ErrorKind::NotFound => {},
        Err(error) => return Err(error),
    }
    security::make_private(&root_handle, true)?;
    // Depth-first streaming retains only ancestor iterators and handles. Each
    // checked name stays pinned until its descendants have been processed.
    let mut stack = vec![(root.to_path_buf(), std::fs::read_dir(root)?, root_handle)];
    while let Some((directory, entries, _guard)) = stack.last_mut() {
        let Some(entry) = entries.next() else { stack.pop(); continue; };
        let entry = entry?;
        let is_directory = if directory == root {
            let Some(kind) = paths::owned_data_entry(&entry.file_name()) else { continue; };
            kind
        } else { entry.file_type()?.is_dir() };
        let path = entry.path();
        let handle = security::pin_owned(&path, is_directory)?;
        security::make_private(&handle, is_directory)?;
        if is_directory { stack.push((path.clone(), std::fs::read_dir(&path)?, handle)); }
    }
    let handle = security::pin_owned(root, true)?;
    write_certificate(root, &handle)?;
    verify(root)
}


#[cfg(test)]
#[path = "../tests/unit/data_root_privacy.rs"]
mod tests;
