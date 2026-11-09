//! Conversion between the two framings of an LZMA (method 14) stream.
//!
//! Inside a ZIP entry, LZMA data is framed as described in APPNOTE.TXT section 5.8:
//!
//! | Offset | Size | Field                                          |
//! |--------|------|------------------------------------------------|
//! | 0      | 1    | LZMA SDK major version (informational only)    |
//! | 1      | 1    | LZMA SDK minor version (informational only)    |
//! | 2      | 2    | Size of the properties that follow (always 5)  |
//! | 4      | 5    | LZMA properties (`lc`/`lp`/`pb` byte, u32 dictionary size) |
//!
//! There is no uncompressed size field. Instead, general purpose bit 1 says whether the stream is terminated by an
//! end-of-stream marker; when it is clear, the stream ends after the entry's uncompressed size.
//!
//! `async-compression`'s LZMA codec instead reads and writes the `.lzma` (`LZMA_Alone`) framing: the same 5 property
//! bytes followed by a u64 uncompressed size, where `u64::MAX` means "unknown, terminated by an end-of-stream
//! marker". The compressed payload after the header is identical in both framings, so only the header needs
//! rewriting.

use std::io;

/// The size of the LZMA properties (`lc`/`lp`/`pb` byte + u32 dictionary size).
const PROPERTIES_LENGTH: usize = 5;

/// The length of the header that precedes LZMA data inside a ZIP entry.
pub(crate) const ZIP_HEADER_LENGTH: usize = 4 + PROPERTIES_LENGTH;

/// The length of the `.lzma` (`LZMA_Alone`) header.
pub(crate) const ALONE_HEADER_LENGTH: usize = PROPERTIES_LENGTH + 8;

/// The `.lzma` uncompressed size value meaning "unknown; the stream ends with an end-of-stream marker".
const UNKNOWN_SIZE: u64 = u64::MAX;

/// liblzma rejects `.lzma` headers that declare an uncompressed size of 256 GiB or more when it auto-detects the
/// format, which is how `async-compression` constructs its LZMA decoder.
#[cfg(feature = "lzma-read")]
const MAX_KNOWN_SIZE: u64 = 1 << 38;

/// The LZMA SDK version written to the ZIP LZMA header. Readers ignore it; this matches CPython's `zipfile`.
#[cfg(feature = "lzma-write")]
const SDK_VERSION: [u8; 2] = [9, 4];

/// Converts the ZIP LZMA header that precedes an entry's compressed data to a `.lzma` header.
///
/// `uncompressed_size` must be `None` when the entry has general purpose bit 1 set (the stream ends with an
/// end-of-stream marker), and the entry's uncompressed size otherwise.
#[cfg(feature = "lzma-read")]
pub(crate) fn zip_to_alone_header(
    zip: &[u8; ZIP_HEADER_LENGTH],
    uncompressed_size: Option<u64>,
) -> io::Result<[u8; ALONE_HEADER_LENGTH]> {
    let properties_length = u16::from_le_bytes([zip[2], zip[3]]);
    if usize::from(properties_length) != PROPERTIES_LENGTH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported LZMA properties length {properties_length}, expected {PROPERTIES_LENGTH}"),
        ));
    }

    let size = match uncompressed_size {
        None => UNKNOWN_SIZE,
        Some(size) if size < MAX_KNOWN_SIZE => size,
        Some(_) => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "LZMA entries of 256 GiB or more without an end-of-stream marker are not supported",
            ))
        }
    };

    let mut alone = [0; ALONE_HEADER_LENGTH];
    alone[0] = zip[4];
    let dictionary_size = u32::from_le_bytes([zip[5], zip[6], zip[7], zip[8]]);
    alone[1..5].copy_from_slice(&round_up_dictionary_size(dictionary_size).to_le_bytes());
    alone[5..].copy_from_slice(&size.to_le_bytes());
    Ok(alone)
}

/// Rounds a dictionary size up to the nearest `2^n` or `2^n + 2^(n-1)`.
///
/// liblzma only accepts those sizes in `.lzma` headers when it auto-detects the format, while ZIP writers may record
/// any size. Decoding with a dictionary at least as large as the encoder's is always correct, and this grows it by
/// less than a third.
#[cfg(feature = "lzma-read")]
fn round_up_dictionary_size(size: u32) -> u32 {
    // liblzma never allocates less than 4 KiB, so smaller sizes can be rounded freely.
    let size = size.max(4096);
    let high = 1u32 << (31 - size.leading_zeros());
    if size == high {
        return size;
    }
    let mid = high | (high >> 1);
    if size <= mid {
        mid
    } else {
        // Past 3 GiB the next power of two doesn't fit; `u32::MAX` is liblzma's accepted maximum.
        high.saturating_mul(2)
    }
}

/// Converts the `.lzma` header produced by `async-compression`'s LZMA encoder to a ZIP LZMA header.
///
/// The encoder always writes an unknown uncompressed size followed by an end-of-stream marker, so entries written
/// with this header must set general purpose bit 1.
#[cfg(feature = "lzma-write")]
pub(crate) fn alone_to_zip_header(alone: &[u8; ALONE_HEADER_LENGTH]) -> io::Result<[u8; ZIP_HEADER_LENGTH]> {
    if alone[PROPERTIES_LENGTH..] != UNKNOWN_SIZE.to_le_bytes() {
        return Err(io::Error::other("LZMA encoder did not produce an end-of-stream-terminated stream"));
    }

    let mut zip = [0; ZIP_HEADER_LENGTH];
    zip[..2].copy_from_slice(&SDK_VERSION);
    zip[2..4].copy_from_slice(&(PROPERTIES_LENGTH as u16).to_le_bytes());
    zip[4..].copy_from_slice(&alone[..PROPERTIES_LENGTH]);
    Ok(zip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "lzma-read")]
    #[test]
    fn dictionary_sizes_round_up_to_accepted_values() {
        for (size, expected) in [
            (0, 4096),
            (4096, 4096),
            (4097, 6144),
            (6144, 6144),
            (6145, 8192),
            (8 << 20, 8 << 20),
            (5 << 20, 6 << 20),
            (7 << 20, 8 << 20),
            (1 << 31, 1 << 31),
            (3 << 30, 3 << 30),
            ((3 << 30) + 1, u32::MAX),
            (u32::MAX, u32::MAX),
        ] {
            assert_eq!(round_up_dictionary_size(size), expected, "size {size}");
        }
    }

    #[cfg(feature = "lzma-read")]
    #[test]
    fn zip_header_converts_to_alone_header() {
        let zip = [9, 4, 5, 0, 0x5d, 0x00, 0x00, 0x80, 0x00];
        let alone = zip_to_alone_header(&zip, None).unwrap();
        assert_eq!(alone, [0x5d, 0x00, 0x00, 0x80, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);

        let alone = zip_to_alone_header(&zip, Some(0x0102)).unwrap();
        assert_eq!(alone, [0x5d, 0x00, 0x00, 0x80, 0x00, 0x02, 0x01, 0, 0, 0, 0, 0, 0]);

        // A dictionary size liblzma would reject is rounded up.
        let zip = [9, 4, 5, 0, 0x5d, 0x00, 0x00, 0x50, 0x00];
        let alone = zip_to_alone_header(&zip, None).unwrap();
        assert_eq!(alone[1..5], (6u32 << 20).to_le_bytes());
    }

    #[cfg(feature = "lzma-read")]
    #[test]
    fn zip_header_rejects_unexpected_input() {
        let err = zip_to_alone_header(&[9, 4, 6, 0, 0x5d, 0, 0, 0x80, 0], None).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);

        let zip = [9, 4, 5, 0, 0x5d, 0x00, 0x00, 0x80, 0x00];
        let err = zip_to_alone_header(&zip, Some(MAX_KNOWN_SIZE)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
        zip_to_alone_header(&zip, Some(MAX_KNOWN_SIZE - 1)).unwrap();
    }

    #[cfg(feature = "lzma-write")]
    #[test]
    fn alone_header_converts_to_zip_header() {
        let alone = [0x5d, 0x00, 0x00, 0x80, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
        assert_eq!(alone_to_zip_header(&alone).unwrap(), [9, 4, 5, 0, 0x5d, 0x00, 0x00, 0x80, 0x00]);

        let alone = [0x5d, 0x00, 0x00, 0x80, 0x00, 0x02, 0x01, 0, 0, 0, 0, 0, 0];
        alone_to_zip_header(&alone).unwrap_err();
    }
}
