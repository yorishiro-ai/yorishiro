//! Shared binary-data representation for checksums, API keys and signatures.

/// Encodes bytes as lowercase hexadecimal without pulling domain models into callers.
pub(crate) fn hex(bytes: impl AsRef<[u8]>) -> String {
    use std::fmt::Write;

    let bytes = bytes.as_ref();
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // Writing into a String cannot fail.
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}
