//! Volume identity — an identifier stronger than "the folder exists". The
//! developer's backup drives share identical directory structures, so
//! directory presence proves nothing about WHICH drive is mounted; the
//! session gate must catch a substituted volume, not just an absent one.
//!
//! macOS: the volume UUID via `diskutil info -plist` on the mount point.
//! Windows: the volume serial via a direct `GetVolumeInformationW` call (a
//! ~20-line FFI declaration beats a whole windows-crate dependency).
//! A volume whose identity cannot be read yields None — nothing is recorded
//! and presence remains the only verification (honest degradation for
//! filesystems without a stable identity, e.g. some network mounts).

use std::path::Path;

/// The identity of the volume containing `path`, when the platform can say.
pub fn volume_identity(path: &Path) -> Option<String> {
    let root = crate::trash::volume_root_of(path).ok()?;
    platform_identity(&root)
}

#[cfg(target_os = "macos")]
fn platform_identity(root: &Path) -> Option<String> {
    let output = std::process::Command::new("diskutil")
        .args(["info", "-plist"])
        .arg(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    extract_plist_string(&String::from_utf8_lossy(&output.stdout), "VolumeUUID")
}

/// Pulls `<key>NAME</key><string>VALUE</string>` out of plist XML by string
/// search — the one value we need does not justify a plist crate.
#[cfg(target_os = "macos")]
fn extract_plist_string(plist: &str, key: &str) -> Option<String> {
    let key_tag = format!("<key>{key}</key>");
    let after = &plist[plist.find(&key_tag)? + key_tag.len()..];
    let start = after.find("<string>")? + "<string>".len();
    let end = after.find("</string>")?;
    if start >= end {
        return None;
    }
    let value = after[start..end].trim().to_string();
    (!value.is_empty()).then_some(value)
}

#[cfg(windows)]
fn platform_identity(root: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetVolumeInformationW(
            root_path_name: *const u16,
            volume_name_buffer: *mut u16,
            volume_name_size: u32,
            volume_serial_number: *mut u32,
            maximum_component_length: *mut u32,
            file_system_flags: *mut u32,
            file_system_name_buffer: *mut u16,
            file_system_name_size: u32,
        ) -> i32;
    }
    let mut wide: Vec<u16> = root.as_os_str().encode_wide().collect();
    if wide.last() != Some(&u16::from(b'\\')) {
        wide.push(u16::from(b'\\'));
    }
    wide.push(0);
    let mut serial: u32 = 0;
    let ok = unsafe {
        GetVolumeInformationW(
            wide.as_ptr(),
            std::ptr::null_mut(),
            0,
            &mut serial,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
        )
    };
    (ok != 0 && serial != 0).then(|| format!("{serial:08X}"))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn platform_identity(_root: &Path) -> Option<String> {
    None
}

/// What comparing a directory's CURRENT volume identity against the recorded
/// one means. Kept apart from the command shell because it is the gate that
/// catches a different drive mounted at a configured path — the case
/// `volume_identity` exists for, since backup drives share directory
/// structures — and it runs before every destructive operation.
#[derive(Debug, PartialEq, Eq)]
pub enum IdentityCheck {
    /// Nothing was recorded for this directory; the identity is now stored.
    FirstSight,
    Unchanged,
    /// A DIFFERENT volume is mounted here. The record is deliberately left
    /// alone: overwriting it would launder the substitution into the new
    /// normal, and the developer must resolve it.
    Substituted { recorded: String },
}

pub fn check_identity(
    conn: &rusqlite::Connection,
    dir: &str,
    current: &str,
) -> Result<IdentityCheck, String> {
    use rusqlite::OptionalExtension;
    let stored: Option<String> = conn
        .query_row(
            "SELECT identity FROM source_volumes WHERE dir = ?1",
            [dir],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    match stored {
        None => {
            conn.execute(
                "INSERT INTO source_volumes (dir, identity, recorded_at_utc) \
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![dir, current, crate::logging::now_iso_millis()],
            )
            .map_err(|e| e.to_string())?;
            Ok(IdentityCheck::FirstSight)
        }
        Some(recorded) if recorded != current => Ok(IdentityCheck::Substituted { recorded }),
        Some(_) => Ok(IdentityCheck::Unchanged),
    }
}

/// Drops recorded identities for directories that are no longer configured,
/// so removing a source root does not leave a record that would later flag a
/// re-added path as substituted.
pub fn prune_identities(
    conn: &rusqlite::Connection,
    configured: &[String],
) -> Result<u64, String> {
    let mut stmt = conn
        .prepare("SELECT dir FROM source_volumes")
        .map_err(|e| e.to_string())?;
    let recorded: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);
    let mut pruned = 0;
    for dir in recorded {
        if !configured.contains(&dir) {
            conn.execute("DELETE FROM source_volumes WHERE dir = ?1", [&dir])
                .map_err(|e| e.to_string())?;
            pruned += 1;
        }
    }
    Ok(pruned)
}

#[cfg(test)]
mod tests {
    // EXCEPTION to the tests-live-in-tests/ rule (tests-folder conventions,
    // Rust form): the plist string extractor is a private parsing detail —
    // promoting it would widen the surface just to test through it. The
    // public volume_identity is exercised from tests/volume_tests.rs.
    #[cfg(target_os = "macos")]
    #[test]
    fn plist_extraction_finds_the_keyed_string() {
        let plist = r#"<dict>
            <key>VolumeName</key><string>Macintosh HD</string>
            <key>VolumeUUID</key>
            <string>  AAAA-BBBB  </string>
        </dict>"#;
        assert_eq!(
            super::extract_plist_string(plist, "VolumeUUID").as_deref(),
            Some("AAAA-BBBB")
        );
        assert_eq!(super::extract_plist_string(plist, "Missing"), None);
    }
}
