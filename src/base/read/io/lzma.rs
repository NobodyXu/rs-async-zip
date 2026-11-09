//! A reader which presents a ZIP entry's LZMA data in the `.lzma` framing that `async-compression` decodes.

use crate::spec::lzma::{zip_to_alone_header, ALONE_HEADER_LENGTH, ZIP_HEADER_LENGTH};

use std::io;
use std::pin::Pin;
use std::task::{ready, Context, Poll};

use futures_lite::io::{AsyncBufRead, AsyncRead};
use pin_project::pin_project;

/// Replaces the ZIP LZMA header at the start of `inner` with the equivalent `.lzma` header, passing the compressed
/// payload that follows through unchanged.
#[pin_project]
pub(crate) struct ZipLzmaReader<R> {
    #[pin]
    inner: R,
    /// `None` if the stream ends with an end-of-stream marker.
    uncompressed_size: Option<u64>,
    state: State,
}

enum State {
    /// Collecting the ZIP LZMA header from `inner`.
    ZipHeader { header: [u8; ZIP_HEADER_LENGTH], filled: usize },
    /// Yielding the converted header.
    AloneHeader { header: [u8; ALONE_HEADER_LENGTH], consumed: usize },
    /// Passing the payload through.
    Payload,
}

impl<R> ZipLzmaReader<R> {
    pub(crate) fn new(inner: R, uncompressed_size: Option<u64>) -> Self {
        Self { inner, uncompressed_size, state: State::ZipHeader { header: [0; ZIP_HEADER_LENGTH], filled: 0 } }
    }

    pub(crate) fn get_ref(&self) -> &R {
        &self.inner
    }

    pub(crate) fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: AsyncBufRead> AsyncRead for ZipLzmaReader<R> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        let available = ready!(self.as_mut().poll_fill_buf(cx))?;
        let read = available.len().min(buf.len());
        buf[..read].copy_from_slice(&available[..read]);
        self.consume(read);
        Poll::Ready(Ok(read))
    }
}

impl<R: AsyncBufRead> AsyncBufRead for ZipLzmaReader<R> {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<&[u8]>> {
        let mut this = self.project();

        while let State::ZipHeader { header, filled } = this.state {
            let available = ready!(this.inner.as_mut().poll_fill_buf(cx))?;
            if available.is_empty() {
                return Poll::Ready(Err(io::Error::new(io::ErrorKind::UnexpectedEof, "LZMA header was truncated")));
            }
            let read = available.len().min(header.len() - *filled);
            header[*filled..*filled + read].copy_from_slice(&available[..read]);
            this.inner.as_mut().consume(read);
            *filled += read;

            if *filled == header.len() {
                let header = zip_to_alone_header(header, *this.uncompressed_size)?;
                *this.state = State::AloneHeader { header, consumed: 0 };
            }
        }

        if matches!(this.state, State::AloneHeader { header, consumed } if *consumed == header.len()) {
            *this.state = State::Payload;
        }
        if let State::AloneHeader { header, consumed } = this.state {
            return Poll::Ready(Ok(&header[*consumed..]));
        }

        this.inner.poll_fill_buf(cx)
    }

    fn consume(self: Pin<&mut Self>, amt: usize) {
        let this = self.project();
        match this.state {
            State::ZipHeader { .. } => debug_assert_eq!(amt, 0, "consumed bytes before they were filled"),
            State::AloneHeader { header, consumed } => *consumed = (*consumed + amt).min(header.len()),
            State::Payload => this.inner.consume(amt),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_lite::io::{AsyncReadExt, BufReader, Cursor};

    #[tokio::test]
    async fn rewrites_only_the_header() {
        let mut zip = vec![9, 4, 5, 0, 0x5d, 0x00, 0x00, 0x80, 0x00];
        zip.extend_from_slice(b"payload");

        // Read one byte at a time to exercise headers split across reads.
        let inner = BufReader::with_capacity(1, Cursor::new(zip));
        let mut reader = ZipLzmaReader::new(inner, Some(7));
        let mut actual = Vec::new();
        reader.read_to_end(&mut actual).await.unwrap();

        let mut expected = vec![0x5d, 0x00, 0x00, 0x80, 0x00, 7, 0, 0, 0, 0, 0, 0, 0];
        expected.extend_from_slice(b"payload");
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn rejects_truncated_header() {
        let mut reader = ZipLzmaReader::new(Cursor::new([9, 4, 5, 0, 0x5d]), None);
        let err = reader.read_to_end(&mut Vec::new()).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }
}
