// Copyright (c) 2022 Harry [Majored] [hello@majored.pw]
// MIT License (https://github.com/Majored/rs-async-zip/blob/main/LICENSE)

#[cfg(feature = "lzma-read")]
use crate::base::read::io::lzma::ZipLzmaReader;
use crate::error::{Result, ZipError};
use crate::spec::Compression;

use std::pin::Pin;
use std::task::{Context, Poll};

#[cfg(any(
    feature = "deflate-read",
    feature = "bzip2-read",
    feature = "zstd-read",
    feature = "lzma-read",
    feature = "xz-read",
    feature = "deflate64-read"
))]
use async_compression::futures::bufread;
use futures_lite::io::{AsyncBufRead, AsyncRead};
use pin_project::pin_project;

/// What a decoder needs to know about an entry besides its compression method and compressed data.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DecodeInfo {
    pub(crate) compression: Compression,
    /// General purpose bit 1. For LZMA, the stream ends with an end-of-stream marker.
    #[cfg_attr(not(feature = "lzma-read"), allow(dead_code))]
    pub(crate) compression_option_1: bool,
    /// The entry's uncompressed size, if it's known before reading the data.
    #[cfg_attr(not(feature = "lzma-read"), allow(dead_code))]
    pub(crate) uncompressed_size: Option<u64>,
}

/// A wrapping reader which holds concrete types for all respective compression method readers.
#[pin_project(project = CompressedReaderProj)]
pub(crate) enum CompressedReader<R> {
    Stored(#[pin] R),
    #[cfg(feature = "deflate-read")]
    Deflate(#[pin] bufread::DeflateDecoder<R>),
    #[cfg(feature = "deflate64-read")]
    Deflate64(#[pin] bufread::Deflate64Decoder<R>),
    #[cfg(feature = "bzip2-read")]
    Bz(#[pin] bufread::BzDecoder<R>),
    #[cfg(feature = "lzma-read")]
    Lzma(#[pin] bufread::LzmaDecoder<ZipLzmaReader<R>>),
    #[cfg(feature = "zstd-read")]
    Zstd(#[pin] bufread::ZstdDecoder<R>),
    #[cfg(feature = "xz-read")]
    Xz(#[pin] bufread::XzDecoder<R>),
}

impl<R> CompressedReader<R>
where
    R: AsyncBufRead + Unpin,
{
    /// Constructs a new wrapping reader from a generic [`AsyncBufRead`] implementer.
    pub(crate) fn new(reader: R, info: DecodeInfo) -> Result<Self> {
        Ok(match info.compression {
            Compression::Stored => CompressedReader::Stored(reader),
            #[cfg(feature = "deflate-read")]
            Compression::Deflate => CompressedReader::Deflate(bufread::DeflateDecoder::new(reader)),
            #[cfg(feature = "deflate64-read")]
            Compression::Deflate64 => CompressedReader::Deflate64(bufread::Deflate64Decoder::new(reader)),
            #[cfg(feature = "bzip2-read")]
            Compression::Bz => CompressedReader::Bz(bufread::BzDecoder::new(reader)),
            #[cfg(feature = "lzma-read")]
            Compression::Lzma => {
                // Without an end-of-stream marker, the stream can only be terminated by its uncompressed size.
                let uncompressed_size = match (info.compression_option_1, info.uncompressed_size) {
                    (true, _) => None,
                    (false, Some(size)) => Some(size),
                    (false, None) => {
                        return Err(ZipError::FeatureNotSupported(
                            "stream reading LZMA entries with data descriptors & no end-of-stream marker",
                        ))
                    }
                };
                CompressedReader::Lzma(bufread::LzmaDecoder::new(ZipLzmaReader::new(reader, uncompressed_size)))
            }
            #[cfg(feature = "zstd-read")]
            Compression::Zstd => CompressedReader::Zstd(bufread::ZstdDecoder::new(reader)),
            #[cfg(feature = "xz-read")]
            Compression::Xz => CompressedReader::Xz(bufread::XzDecoder::new(reader)),
            #[allow(unreachable_patterns)]
            _ => return Err(ZipError::CompressionNotSupported(info.compression.into())),
        })
    }

    /// Consumes this reader and returns the inner value.
    pub(crate) fn inner(&self) -> &R {
        match self {
            CompressedReader::Stored(inner) => inner,
            #[cfg(feature = "deflate-read")]
            CompressedReader::Deflate(inner) => inner.get_ref(),
            #[cfg(feature = "deflate64-read")]
            CompressedReader::Deflate64(inner) => inner.get_ref(),
            #[cfg(feature = "bzip2-read")]
            CompressedReader::Bz(inner) => inner.get_ref(),
            #[cfg(feature = "lzma-read")]
            CompressedReader::Lzma(inner) => inner.get_ref().get_ref(),
            #[cfg(feature = "zstd-read")]
            CompressedReader::Zstd(inner) => inner.get_ref(),
            #[cfg(feature = "xz-read")]
            CompressedReader::Xz(inner) => inner.get_ref(),
        }
    }

    /// Consumes this reader and returns the inner value.
    pub(crate) fn into_inner(self) -> R {
        match self {
            CompressedReader::Stored(inner) => inner,
            #[cfg(feature = "deflate-read")]
            CompressedReader::Deflate(inner) => inner.into_inner(),
            #[cfg(feature = "deflate64-read")]
            CompressedReader::Deflate64(inner) => inner.into_inner(),
            #[cfg(feature = "bzip2-read")]
            CompressedReader::Bz(inner) => inner.into_inner(),
            #[cfg(feature = "lzma-read")]
            CompressedReader::Lzma(inner) => inner.into_inner().into_inner(),
            #[cfg(feature = "zstd-read")]
            CompressedReader::Zstd(inner) => inner.into_inner(),
            #[cfg(feature = "xz-read")]
            CompressedReader::Xz(inner) => inner.into_inner(),
        }
    }
}

impl<R> AsyncRead for CompressedReader<R>
where
    R: AsyncBufRead + Unpin,
{
    fn poll_read(self: Pin<&mut Self>, c: &mut Context<'_>, b: &mut [u8]) -> Poll<std::io::Result<usize>> {
        match self.project() {
            CompressedReaderProj::Stored(inner) => inner.poll_read(c, b),
            #[cfg(feature = "deflate-read")]
            CompressedReaderProj::Deflate(inner) => inner.poll_read(c, b),
            #[cfg(feature = "deflate64-read")]
            CompressedReaderProj::Deflate64(inner) => inner.poll_read(c, b),
            #[cfg(feature = "bzip2-read")]
            CompressedReaderProj::Bz(inner) => inner.poll_read(c, b),
            #[cfg(feature = "lzma-read")]
            CompressedReaderProj::Lzma(inner) => inner.poll_read(c, b),
            #[cfg(feature = "zstd-read")]
            CompressedReaderProj::Zstd(inner) => inner.poll_read(c, b),
            #[cfg(feature = "xz-read")]
            CompressedReaderProj::Xz(inner) => inner.poll_read(c, b),
        }
    }
}
