macro_rules! compression_tests {
    ($name:ident, $read:literal, $write:literal, $variant:ident, $decoder:ident, $method:literal) => {
        compression_tests!($name, $read, $write, $variant, $decoder, $method, |payload: &[u8]| payload.to_vec());
    };
    ($name:ident, $read:literal, $write:literal, $variant:ident, $decoder:ident, $method:literal, $to_decoder_input:expr) => {
        #[cfg(any(feature = $read, feature = $write))]
        mod $name {
            #[cfg(feature = $write)]
            use async_zip::base::read;
            use async_zip::base::write::ZipFileWriter;
            #[cfg(any(not(feature = $read), not(feature = $write)))]
            use async_zip::error::ZipError;
            use async_zip::{Compression, ZipEntryBuilder};
            use futures_lite::io::Cursor;

            #[cfg(not(feature = $write))]
            #[tokio::test]
            async fn rejects_writes_without_emitting_headers() {
                for mode in 0..3 {
                    let mut buffer = Vec::new();
                    let mut writer = ZipFileWriter::new(Cursor::new(&mut buffer));
                    let entry = ZipEntryBuilder::new("file".into(), Compression::$variant);
                    let result = match mode {
                        0 => writer.write_entry_whole(entry, b"data").await,
                        1 => writer.write_entry_stream(entry).await.map(|_| ()),
                        _ => writer.write_entry_seekable(entry).await.map(|_| ()),
                    };
                    match result {
                        Err(ZipError::FeatureNotSupported(message)) => {
                            assert_eq!(message, concat!(stringify!($variant), " writing"));
                        }
                        _ => panic!("expected an unsupported writer error"),
                    }
                    assert!(buffer.is_empty());
                }
            }

            #[cfg(feature = $write)]
            #[tokio::test]
            async fn writes_all_entry_types() {
                use futures_lite::io::{AsyncReadExt, AsyncWriteExt};

                let contents = b"repeated data ".repeat(128);
                for mode in 0..3 {
                    let mut writer = ZipFileWriter::new(Cursor::new(Vec::new()));
                    let entry = ZipEntryBuilder::new("file".into(), Compression::$variant)
                        .deflate_option(async_zip::DeflateOption::Normal);
                    match mode {
                        0 => writer.write_entry_whole(entry, &contents).await.unwrap(),
                        1 => {
                            let mut entry = writer.write_entry_stream(entry).await.unwrap();
                            entry.write_all(&contents).await.unwrap();
                            entry.close().await.unwrap();
                        }
                        _ => {
                            let mut entry = writer.write_entry_seekable(entry).await.unwrap();
                            entry.write_all(&contents).await.unwrap();
                            entry.close().await.unwrap();
                        }
                    }
                    let buffer = writer.close().await.unwrap().into_inner();
                    let reader = read::mem::ZipFileReader::new(buffer.clone()).await.unwrap();
                    let entry = &reader.file().entries()[0];
                    assert_eq!(u16::from(entry.compression()), $method);
                    // General purpose bit 1 announces LZMA's end-of-stream marker and is clear for other methods.
                    let flags = u16::from_le_bytes(buffer[6..8].try_into().unwrap());
                    assert_eq!(flags & 0x2 != 0, $method == 14);
                    assert_eq!(entry.uncompressed_size(), contents.len() as u64);

                    // Decode the payload independently of this crate's read feature.
                    let name_len = u16::from_le_bytes(buffer[26..28].try_into().unwrap()) as usize;
                    let extra_len = u16::from_le_bytes(buffer[28..30].try_into().unwrap()) as usize;
                    let start = 30 + name_len + extra_len;
                    let end = start + entry.compressed_size() as usize;
                    let input = ($to_decoder_input)(&buffer[start..end]);
                    let mut decoder = async_compression::futures::bufread::$decoder::new(input.as_slice());
                    let mut actual = Vec::new();
                    decoder.read_to_end(&mut actual).await.unwrap();
                    assert_eq!(actual, contents);
                    assert_eq!(entry.crc32(), crc32fast::hash(&actual));

                    #[cfg(feature = $read)]
                    {
                        let mut entry = reader.reader_with_entry(0).await.unwrap();
                        let mut actual = Vec::new();
                        entry.read_to_end_checked(&mut actual).await.unwrap();
                        assert_eq!(actual, contents);
                    }

                    #[cfg(not(feature = $read))]
                    {
                        assert!(matches!(
                            reader.reader_without_entry(0).await,
                            Err(ZipError::CompressionNotSupported($method))
                        ));
                        assert!(matches!(
                            reader.reader_with_entry(0).await,
                            Err(ZipError::CompressionNotSupported($method))
                        ));
                        let mut reader = read::seek::ZipFileReader::new(Cursor::new(&buffer)).await.unwrap();
                        assert!(matches!(
                            reader.reader_without_entry(0).await,
                            Err(ZipError::CompressionNotSupported($method))
                        ));
                        assert!(matches!(
                            reader.reader_with_entry(0).await,
                            Err(ZipError::CompressionNotSupported($method))
                        ));
                        assert!(matches!(reader.into_entry(0).await, Err(ZipError::CompressionNotSupported($method))));
                        let reader = read::stream::ZipFileReader::new(buffer.as_slice());
                        assert!(matches!(
                            reader.next_without_entry().await,
                            Err(ZipError::CompressionNotSupported($method))
                        ));
                        let reader = read::stream::ZipFileReader::new(buffer.as_slice());
                        assert!(matches!(
                            reader.next_with_entry().await,
                            Err(ZipError::CompressionNotSupported($method))
                        ));
                    }
                }
            }
        }
    };
}

compression_tests!(deflate, "deflate-read", "deflate-write", Deflate, DeflateDecoder, 8);
compression_tests!(bzip2, "bzip2-read", "bzip2-write", Bz, BzDecoder, 12);
compression_tests!(lzma, "lzma-read", "lzma-write", Lzma, LzmaDecoder, 14, |payload: &[u8]| {
    // ZIP LZMA data starts with a 2-byte version, a 2-byte properties length and the 5-byte properties. Rebuild the
    // `.lzma` header the decoder expects: the properties followed by an unknown (end-marker-terminated) size.
    assert_eq!(payload[2..4], [5, 0]);
    let mut alone = payload[4..9].to_vec();
    alone.extend_from_slice(&u64::MAX.to_le_bytes());
    alone.extend_from_slice(&payload[9..]);
    alone
});
compression_tests!(zstd, "zstd-read", "zstd-write", Zstd, ZstdDecoder, 93);
compression_tests!(xz, "xz-read", "xz-write", Xz, XzDecoder, 95);

/// Checks LZMA entries against an independent implementation (the `zip` crate, backed by `lzma-rust2`).
#[cfg(feature = "lzma-write")]
#[tokio::test]
async fn lzma_entries_are_readable_by_other_implementations() {
    use async_zip::base::write::ZipFileWriter;
    use async_zip::{Compression, ZipEntryBuilder};
    use futures_lite::io::{AsyncWriteExt, Cursor};
    use std::io::Read;

    let contents = b"repeated data ".repeat(128);
    let mut writer = ZipFileWriter::new(Cursor::new(Vec::new()));
    for (mode, name) in ["whole", "stream", "seekable"].into_iter().enumerate() {
        let entry = ZipEntryBuilder::new(name.into(), Compression::Lzma);
        match mode {
            0 => writer.write_entry_whole(entry, &contents).await.unwrap(),
            1 => {
                let mut entry = writer.write_entry_stream(entry).await.unwrap();
                entry.write_all(&contents).await.unwrap();
                entry.close().await.unwrap();
            }
            _ => {
                let mut entry = writer.write_entry_seekable(entry).await.unwrap();
                entry.write_all(&contents).await.unwrap();
                entry.close().await.unwrap();
            }
        }
    }
    let buffer = writer.close().await.unwrap().into_inner();

    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(buffer)).unwrap();
    for name in ["whole", "stream", "seekable"] {
        let mut file = archive.by_name(name).unwrap();
        assert_eq!(file.compression(), zip::CompressionMethod::Lzma);
        let mut actual = Vec::new();
        file.read_to_end(&mut actual).unwrap();
        assert_eq!(actual, contents, "for {name}");
    }
}
