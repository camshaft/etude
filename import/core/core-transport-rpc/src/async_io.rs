//! RPC implementation over tokio's AsyncRead and AsyncWrite traits

use crate::traits::{Reader, Writer};
use bytes::BytesMut;
use upstream_buffer::{
    reader::{self, Storage as _, storage::Infallible as _},
    writer,
};
use std::{future::Future, pin::Pin};
use tokio::io::{self, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub struct AsyncReader<S> {
    read: usize,
    buffer: BytesMut,
    stream: S,
    is_stream_open: bool,
}

impl<S> AsyncReader<S> {
    /// Returns the inner stream, consuming this reader.
    ///
    /// Any buffered data that has been read from the stream but not yet
    /// consumed is discarded.
    #[inline]
    pub fn into_inner(self) -> S {
        self.stream
    }
}

impl<S> AsyncReader<S>
where
    S: AsyncRead + Unpin,
{
    #[inline]
    pub fn new(stream: S) -> Self {
        Self {
            read: 0,
            buffer: BytesMut::new(),
            stream,
            is_stream_open: true,
        }
    }

    #[inline]
    async fn read_chunk<Payload>(&mut self, payload: &mut Payload) -> io::Result<usize>
    where
        Payload: writer::Storage,
    {
        loop {
            // check if the payload has any remaining capacity
            if !payload.has_remaining_capacity() {
                return Ok(0);
            }

            // drain what we've buffered so far
            if !self.buffer.is_empty() {
                let mut buffer = self.buffer.track_read();
                buffer.infallible_copy_into(payload);
                return Ok(buffer.consumed_len());
            }

            let read_len = self.fill_buffer().await?;

            // if we got to the end of the stream and the buffer is empty then we're done
            if read_len == 0 && self.buffer.is_empty() {
                return Ok(0);
            }
        }
    }

    #[inline]
    async fn fill_buffer(&mut self) -> io::Result<usize> {
        if !self.is_stream_open {
            return Ok(0);
        }

        // check if we need to grow the buffer
        if self.buffer.is_empty() {
            let (min_remaining, allocation_target) = if self.read > u16::MAX as usize * 4 {
                // if we've read at least 4 64k chunks, then it's likely we're going to keep getting large chunks
                // so increase our allocation strategy to get more throughput.
                (1024 * 8, u16::MAX as usize * 2)
            } else {
                (1024, u16::MAX as usize)
            };

            // use `spare_capacity_mut` to get the current allocation size
            if self.buffer.spare_capacity_mut().len() < min_remaining {
                self.buffer = BytesMut::with_capacity(allocation_target);
            }
        }

        let read_len = Pin::new(&mut self.stream)
            .read_buf(&mut self.buffer)
            .await?;

        // check if the stream was terminated
        if read_len == 0 {
            self.is_stream_open = false;
            Ok(read_len)
        } else {
            self.read += read_len;
            Ok(read_len)
        }
    }
}

impl<S> Reader for AsyncReader<S>
where
    S: 'static + AsyncRead + Send + Unpin,
{
    #[inline]
    fn read_chunk<Payload>(
        &mut self,
        payload: &mut Payload,
    ) -> impl Future<Output = io::Result<usize>> + Send
    where
        Payload: writer::Storage + Send,
    {
        Self::read_chunk(self, payload)
    }
}

pub struct AsyncWriter<S> {
    total: usize,
    stream: S,
}

impl<S> AsyncWriter<S> {
    /// Returns the inner stream, consuming this writer.
    #[inline]
    pub fn into_inner(self) -> S {
        self.stream
    }
}

impl<S> AsyncWriter<S>
where
    S: AsyncWrite + Unpin,
{
    #[inline]
    pub fn new(stream: S) -> Self {
        Self { total: 0, stream }
    }
}

impl<S> Writer for AsyncWriter<S>
where
    S: 'static + AsyncWrite + Send + Unpin,
{
    #[inline]
    fn write_chunk<Payload>(
        &mut self,
        payload: &mut Payload,
    ) -> impl Future<Output = io::Result<usize>> + Send
    where
        Payload: reader::storage::Infallible + Send,
    {
        async move {
            let chunk = payload.infallible_read_chunk(usize::MAX);
            if chunk.is_empty() {
                return Ok(0);
            }
            self.stream.write_all(&chunk).await?;
            self.total += chunk.len();
            Ok(chunk.len())
        }
    }

    fn keep_alive(&mut self, enabled: bool) {
        // TODO
        let _ = enabled;
    }

    #[inline]
    fn finish(mut self) -> impl Future<Output = io::Result<()>> + Send {
        async move { self.stream.shutdown().await }
    }

    #[inline]
    fn write_to_end<Payload>(
        mut self,
        mut payload: Payload,
    ) -> impl Future<Output = std::io::Result<usize>>
    where
        Payload: reader::storage::Infallible,
    {
        async move {
            // TODO Add a method in s2n-quic's buffer impl to make this more efficient.
            //      Due to lifetimes, it currently needs to process a chunk at a time.
            loop {
                let chunk = payload.infallible_read_chunk(usize::MAX);
                if chunk.is_empty() {
                    break;
                }
                self.stream.write_all(&chunk).await?;
                self.total += chunk.len();
            }

            let total = self.total;

            self.finish().await?;

            Ok(total)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[tokio::test]
    async fn reader_writer() {
        let (reader, writer) = duplex(1024);
        let reader = AsyncReader::new(reader);
        let writer = AsyncWriter::new(writer);

        let request = &b"hello world!"[..];
        let written = writer.write_to_end(request).await.unwrap();
        assert_eq!(written, request.len());

        let bytes = reader.read_to_bytes().await.unwrap();
        assert_eq!(bytes, request);
    }
}
