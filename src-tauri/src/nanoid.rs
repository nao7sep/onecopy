// A minimal, hand-rolled nanoid: generates the discriminator used to name an
// atomic write's temp file (`<stem>-<nanoid>.tmp`, see the derived-filename
// grammar in the storage-path-conventions). Generated in the Rust core rather
// than the webview so no IPC parameter exists solely to carry a random token —
// and without pulling the full `nanoid` crate into the core for one call site.
//
// Alphabet: the 64 URL-safe nanoid characters, A-Za-z0-9_-. Because 64 is a
// power of two dividing 256 evenly, masking a random byte with 0x3F (its
// bottom 6 bits) selects a uniformly random alphabet index — no bias, and no
// rejection sampling needed, unlike an alphabet whose size does not divide
// 256 evenly.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-";

// Matches the frontend nanoid's default length.
const LENGTH: usize = 21;

// Generates a fresh 21-character nanoid. A missing system random source stops
// only the operation that needed a collision-resistant private filename.
pub fn generate() -> Result<String, String> {
    let mut bytes = [0u8; LENGTH];
    getrandom::fill(&mut bytes)
        .map_err(|error| format!("system random source unavailable: {error}"))?;
    Ok(bytes
        .iter()
        .map(|b| ALPHABET[(b & 0x3F) as usize] as char)
        .collect())
}

#[cfg(test)]
// EXCEPTION to tests-folder conventions: checks the identifier length of a
// module that is private to the crate.
#[path = "../tests/unit/nanoid.rs"]
mod tests;
