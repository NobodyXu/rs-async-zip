// Copyright (c) 2022 Harry [Majored] [hello@majored.pw]
// MIT License (https://github.com/Majored/rs-async-zip/blob/main/LICENSE)

use crate::base::read::io::compressed::{CompressedReader, DecodeInfo};
use crate::spec::Compression;

compressed_test_helper!(stored_test, Compression::Stored, "foo bar", "foo bar");

#[cfg(feature = "deflate-read")]
compressed_test_helper!(deflate_test, Compression::Deflate, "foo bar", include_bytes!("deflate.data"));

#[cfg(feature = "bzip2-read")]
compressed_test_helper!(bz_test, Compression::Bz, "foo bar", include_bytes!("bzip2.data"));

#[cfg(feature = "lzma-read")]
// ZIP-framed LZMA as written by CPython's zipfile: LZMA SDK header + end-of-stream marker.
compressed_test_helper!(lzma_test, Compression::Lzma, "foo bar", include_bytes!("lzma.data"));

/// ZIP-framed LZMA without an end-of-stream marker (general purpose bit 1 clear), which ends after the known size.
#[cfg(feature = "lzma-read")]
#[tokio::test]
async fn lzma_without_end_marker_test() {
    use futures_lite::io::{AsyncReadExt, Cursor};

    let data = include_bytes!("lzma_no_eos.data");
    let info = DecodeInfo { compression: Compression::Lzma, compression_option_1: false, uncompressed_size: Some(7) };
    let mut reader = CompressedReader::new(Cursor::new(data), info).unwrap();
    let mut read_data = String::new();
    reader.read_to_string(&mut read_data).await.unwrap();
    assert_eq!(read_data, "foo bar");
}

/// ZIP writers may record any dictionary size, including ones liblzma rejects in `.lzma` headers.
#[cfg(feature = "lzma-read")]
#[tokio::test]
async fn lzma_with_arbitrary_dictionary_size_test() {
    use futures_lite::io::{AsyncReadExt, Cursor};

    let mut data = include_bytes!("lzma.data").to_vec();
    data[5..9].copy_from_slice(&(5u32 << 20).to_le_bytes());
    let info = DecodeInfo { compression: Compression::Lzma, compression_option_1: true, uncompressed_size: None };
    let mut reader = CompressedReader::new(Cursor::new(data), info).unwrap();
    let mut read_data = String::new();
    reader.read_to_string(&mut read_data).await.unwrap();
    assert_eq!(read_data, "foo bar");
}

/// Without an end-of-stream marker or a known size, there's no way to tell where the LZMA stream ends.
#[cfg(feature = "lzma-read")]
#[test]
fn lzma_without_end_marker_or_size_is_rejected() {
    use futures_lite::io::Cursor;

    let info = DecodeInfo { compression: Compression::Lzma, compression_option_1: false, uncompressed_size: None };
    let result = CompressedReader::new(Cursor::new(include_bytes!("lzma_no_eos.data")), info);
    assert!(matches!(result, Err(crate::error::ZipError::FeatureNotSupported(_))));
}

#[cfg(feature = "zstd-read")]
compressed_test_helper!(zstd_test, Compression::Zstd, "foo bar", include_bytes!("zstd.data"));

#[cfg(feature = "xz-read")]
compressed_test_helper!(xz_test, Compression::Xz, "foo bar", include_bytes!("xz.data"));

/// A helper macro for generating a CompressedReader test using a specific compression method.
macro_rules! compressed_test_helper {
    ($name:ident, $typ:expr, $data_raw:expr, $data:expr) => {
        #[cfg(test)]
        #[tokio::test]
        async fn $name() {
            use futures_lite::io::{AsyncReadExt, Cursor};

            let data = $data;
            let data_raw = $data_raw;

            let cursor = Cursor::new(data);
            // Every fixture ends with an end-of-stream marker where the method has one.
            let info = DecodeInfo { compression: $typ, compression_option_1: true, uncompressed_size: None };
            let mut reader = CompressedReader::new(cursor, info).unwrap();

            let mut read_data = String::new();
            reader.read_to_string(&mut read_data).await.expect("read into CompressedReader failed");

            assert_eq!(read_data, data_raw);
        }
    };
}

use compressed_test_helper;
