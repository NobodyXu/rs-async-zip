//! A writer which turns the `.lzma` stream that `async-compression` encodes into a ZIP entry's LZMA data.

use crate::spec::lzma::{alone_to_zip_header, ALONE_HEADER_LENGTH, ZIP_HEADER_LENGTH};

use std::io;
use std::pin::Pin;
use std::task::{ready, Context, Poll};

use futures_lite::io::AsyncWrite;

/// Replaces the `.lzma` header at the start of the written data with the equivalent ZIP LZMA header, passing the
/// compressed payload that follows through unchanged.
///
/// The entry must set general purpose bit 1, since the encoder terminates its stream with an end-of-stream marker.
pub(crate) struct ZipLzmaWriter<W> {
    inner: W,
    state: State,
}

enum State {
    /// Collecting the `.lzma` header from the encoder.
    AloneHeader { header: [u8; ALONE_HEADER_LENGTH], filled: usize },
    /// Writing the converted header to `inner`.
    ZipHeader { header: [u8; ZIP_HEADER_LENGTH], written: usize },
    /// Passing the payload through.
    Payload,
}

impl<W> ZipLzmaWriter<W> {
    pub(crate) fn new(inner: W) -> Self {
        Self { inner, state: State::AloneHeader { header: [0; ALONE_HEADER_LENGTH], filled: 0 } }
    }

    pub(crate) fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: AsyncWrite + Unpin> ZipLzmaWriter<W> {
    /// Writes out the converted header, if it's ready and hasn't been written yet.
    fn poll_write_header(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while let State::ZipHeader { header, written } = &mut self.state {
            if *written == header.len() {
                self.state = State::Payload;
                break;
            }
            let n = ready!(Pin::new(&mut self.inner).poll_write(cx, &header[*written..]))?;
            if n == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
            *written += n;
        }
        Poll::Ready(Ok(()))
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for ZipLzmaWriter<W> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let this = &mut *self;
        if let State::AloneHeader { header, filled } = &mut this.state {
            let n = buf.len().min(header.len() - *filled);
            header[*filled..*filled + n].copy_from_slice(&buf[..n]);
            *filled += n;
            if *filled == header.len() {
                this.state = State::ZipHeader { header: alone_to_zip_header(header)?, written: 0 };
            }
            return Poll::Ready(Ok(n));
        }

        ready!(this.poll_write_header(cx))?;
        Pin::new(&mut this.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        ready!(this.poll_write_header(cx))?;
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        if matches!(this.state, State::AloneHeader { .. }) {
            return Poll::Ready(Err(io::Error::other("LZMA encoder finished without writing a complete header")));
        }
        ready!(this.poll_write_header(cx))?;
        Pin::new(&mut this.inner).poll_close(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_lite::io::{AsyncWriteExt, Cursor};

    #[tokio::test]
    async fn rewrites_only_the_header() {
        let mut alone = vec![0x5d, 0x00, 0x00, 0x80, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
        alone.extend_from_slice(b"payload");

        let mut writer = ZipLzmaWriter::new(Cursor::new(Vec::new()));
        // Write one byte at a time to exercise headers split across writes.
        for byte in &alone {
            writer.write_all(std::slice::from_ref(byte)).await.unwrap();
        }
        writer.close().await.unwrap();

        let mut expected = vec![9, 4, 5, 0, 0x5d, 0x00, 0x00, 0x80, 0x00];
        expected.extend_from_slice(b"payload");
        assert_eq!(writer.into_inner().into_inner(), expected);
    }

    #[tokio::test]
    async fn rejects_truncated_header() {
        let mut writer = ZipLzmaWriter::new(Cursor::new(Vec::new()));
        writer.write_all(&[0x5d, 0x00, 0x00]).await.unwrap();
        writer.close().await.unwrap_err();
    }
}
