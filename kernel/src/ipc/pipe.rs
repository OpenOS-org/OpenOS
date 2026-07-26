//! Unix-like pipe implementation for inter-process communication.
//!
//! A pipe is a unidirectional byte stream with a fixed-capacity ring buffer.
//! Data written to the `PipeWriter` end can be read from the `PipeReader` end.
//!
//! ## Semantics
//!
//! - **Read on empty pipe**: returns 0 bytes (or `WouldBlock` for non-blocking).
//! - **Read with writer dropped**: returns 0 bytes (EOF).
//! - **Write on full pipe**: blocks until space is available (or `WouldBlock`).
//! - **Write with reader dropped**: returns `EPIPE` error.
//!
//! ## Thread safety
//!
//! The `PipeBuffer` is protected by a `spin::Mutex`. Both `PipeReader` and
//! `PipeWriter` hold an `Arc<Mutex<PipeBuffer>>` to allow sharing across tasks.

use alloc::collections::VecDeque;
use alloc::sync::Arc;

use spin::Mutex;

/// Default pipe capacity in bytes (4 KiB).
const DEFAULT_PIPE_CAPACITY: usize = 4096;

/// Error returned when writing to a pipe whose read end has been closed.
const EPIPE: i64 = -32;

/// The shared state of a pipe: a ring buffer with bookkeeping for open ends.
pub struct PipeBuffer {
    /// Ring buffer holding the byte stream.
    buffer: VecDeque<u8>,
    /// Maximum number of bytes the buffer can hold.
    capacity: usize,
    /// Whether the read end is still open.
    reader_open: bool,
    /// Whether the write end is still open.
    writer_open: bool,
}

/// The read end of a pipe.
///
/// Dropping a `PipeReader` signals EOF to any future reads and allows
/// `PipeWriter::write` to detect that the reader is gone.
pub struct PipeReader {
    inner: Arc<Mutex<PipeBuffer>>,
}

/// The write end of a pipe.
///
/// Dropping a `PipeWriter` signals EOF to `PipeReader::read` (returns 0 bytes).
pub struct PipeWriter {
    inner: Arc<Mutex<PipeBuffer>>,
}

/// Create a new pipe pair with the default capacity (4 KiB).
///
/// Returns `(PipeReader, PipeWriter)`.
#[must_use]
pub fn create_pipe() -> (PipeReader, PipeWriter) {
    create_pipe_with_capacity(DEFAULT_PIPE_CAPACITY)
}

/// Create a new pipe pair with the specified capacity.
///
/// Returns `(PipeReader, PipeWriter)`.
#[must_use]
pub fn create_pipe_with_capacity(capacity: usize) -> (PipeReader, PipeWriter) {
    let inner = Arc::new(Mutex::new(PipeBuffer {
        buffer: VecDeque::with_capacity(capacity),
        capacity,
        reader_open: true,
        writer_open: true,
    }));
    (
        PipeReader {
            inner: Arc::clone(&inner),
        },
        PipeWriter { inner },
    )
}

impl PipeReader {
    /// Read up to `buf.len()` bytes from the pipe into `buf`.
    ///
    /// Returns the number of bytes read. Returns 0 if:
    /// - The buffer is empty and the writer has been dropped (EOF).
    /// - The buffer is empty and the writer is still open (no data available).
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned (should not happen in normal operation).
    pub fn read(&self, buf: &mut [u8]) -> usize {
        if buf.is_empty() {
            return 0;
        }

        let mut pipe = self.inner.lock();

        // If the buffer is empty, check if the writer has been dropped (EOF).
        if pipe.buffer.is_empty() {
            if pipe.writer_open {
                // Writer still open, no data: return 0 bytes (caller should retry).
                return 0;
            }
            // Writer dropped, buffer empty: permanent EOF.
            return 0;
        }

        // Copy as many bytes as available, up to the caller's buffer size.
        let to_read = buf.len().min(pipe.buffer.len());
        for (i, byte) in pipe.buffer.drain(..to_read).enumerate() {
            buf[i] = byte;
        }
        to_read
    }

    /// Check whether the write end has been dropped (EOF condition).
    #[must_use]
    pub fn is_eof(&self) -> bool {
        let pipe = self.inner.lock();
        !pipe.writer_open && pipe.buffer.is_empty()
    }

    /// Check whether the pipe has data available to read (for `poll`).
    ///
    /// Returns `true` if the buffer is non-empty or the writer has been dropped
    /// (EOF), both of which mean a `read` call would return immediately.
    #[must_use]
    pub fn is_readable(&self) -> bool {
        let pipe = self.inner.lock();
        !pipe.buffer.is_empty() || !pipe.writer_open
    }
}

impl PipeWriter {
    /// Write bytes from `data` into the pipe.
    ///
    /// Returns the number of bytes actually written. If the pipe is full,
    /// writes as many bytes as will fit and returns that count.
    ///
    /// Returns `Err(EPIPE)` if the read end has been dropped.
    pub fn write(&self, data: &[u8]) -> Result<usize, i64> {
        if data.is_empty() {
            return Ok(0);
        }

        let mut pipe = self.inner.lock();

        if !pipe.reader_open {
            return Err(EPIPE);
        }

        let available = pipe.capacity - pipe.buffer.len();
        if available == 0 {
            return Ok(0);
        }

        let to_write = data.len().min(available);
        for &byte in &data[..to_write] {
            pipe.buffer.push_back(byte);
        }
        Ok(to_write)
    }

    /// Check whether the read end has been dropped.
    #[must_use]
    pub fn is_broken(&self) -> bool {
        let pipe = self.inner.lock();
        !pipe.reader_open
    }

    /// Check whether the pipe has space available to write (for `poll`).
    ///
    /// Returns `true` if the buffer has free capacity. A broken pipe (reader
    /// dropped) is reported via `POLLERR`, not `POLLOUT`.
    #[must_use]
    pub fn is_writable(&self) -> bool {
        let pipe = self.inner.lock();
        pipe.buffer.len() < pipe.capacity
    }
}

impl Drop for PipeReader {
    fn drop(&mut self) {
        let mut pipe = self.inner.lock();
        pipe.reader_open = false;
    }
}

impl Drop for PipeWriter {
    fn drop(&mut self) {
        let mut pipe = self.inner.lock();
        pipe.writer_open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_pipe() {
        let (reader, writer) = create_pipe();
        assert!(!reader.is_eof());
        assert!(!writer.is_broken());
    }

    #[test]
    fn test_basic_write_read() {
        let (reader, writer) = create_pipe();
        let data = b"hello";
        let written = writer.write(data).unwrap();
        assert_eq!(written, 5);

        let mut buf = [0u8; 16];
        let read = reader.read(&mut buf);
        assert_eq!(read, 5);
        assert_eq!(&buf[..5], b"hello");
    }

    #[test]
    fn test_read_empty_returns_zero() {
        let (reader, _writer) = create_pipe();
        let mut buf = [0u8; 16];
        let read = reader.read(&mut buf);
        assert_eq!(read, 0);
    }

    #[test]
    fn test_read_empty_buf_returns_zero() {
        let (reader, writer) = create_pipe();
        writer.write(b"abc").unwrap();
        let mut buf = [];
        let read = reader.read(&mut buf);
        assert_eq!(read, 0);
    }

    #[test]
    fn test_eof_when_writer_dropped() {
        let (reader, writer) = create_pipe();
        writer.write(b"data").unwrap();
        drop(writer);

        // First read should return the buffered data.
        let mut buf = [0u8; 16];
        let read = reader.read(&mut buf);
        assert_eq!(read, 4);
        assert_eq!(&buf[..4], b"data");

        // Second read should return 0 (EOF).
        let read = reader.read(&mut buf);
        assert_eq!(read, 0);
        assert!(reader.is_eof());
    }

    #[test]
    fn test_epipe_when_reader_dropped() {
        let (reader, writer) = create_pipe();
        drop(reader);

        let result = writer.write(b"data");
        assert_eq!(result, Err(EPIPE));
        assert!(writer.is_broken());
    }

    #[test]
    fn test_write_partial_when_full() {
        let (reader, writer) = create_pipe_with_capacity(4);

        // Fill the pipe.
        let written = writer.write(b"abcdefgh").unwrap();
        assert_eq!(written, 4);

        // Read all data.
        let mut buf = [0u8; 8];
        let read = reader.read(&mut buf);
        assert_eq!(read, 4);
        assert_eq!(&buf[..4], b"abcd");
    }

    #[test]
    fn test_write_returns_zero_when_full() {
        let (_reader, writer) = create_pipe_with_capacity(4);

        // Fill the pipe.
        writer.write(b"abcd").unwrap();

        // Writing to a full pipe returns 0 bytes written.
        let written = writer.write(b"e").unwrap();
        assert_eq!(written, 0);
    }

    #[test]
    fn test_write_empty_returns_zero() {
        let (_reader, writer) = create_pipe();
        let written = writer.write(b"").unwrap();
        assert_eq!(written, 0);
    }

    #[test]
    fn test_multiple_write_read_cycles() {
        let (reader, writer) = create_pipe_with_capacity(8);

        writer.write(b"abc").unwrap();
        writer.write(b"def").unwrap();

        let mut buf = [0u8; 16];
        let read = reader.read(&mut buf);
        assert_eq!(read, 6);
        assert_eq!(&buf[..6], b"abcdef");

        writer.write(b"gh").unwrap();
        let read = reader.read(&mut buf);
        assert_eq!(read, 2);
        assert_eq!(&buf[..2], b"gh");
    }

    #[test]
    fn test_read_less_than_available() {
        let (reader, writer) = create_pipe();
        writer.write(b"hello world").unwrap();

        let mut buf = [0u8; 5];
        let read = reader.read(&mut buf);
        assert_eq!(read, 5);
        assert_eq!(&buf, b"hello");

        let read = reader.read(&mut buf);
        assert_eq!(read, 5);
        assert_eq!(&buf, b" worl");

        let mut buf2 = [0u8; 8];
        let read = reader.read(&mut buf2);
        assert_eq!(read, 1);
        assert_eq!(&buf2[..1], b"d");
    }

    #[test]
    fn test_default_capacity() {
        let (_reader, writer) = create_pipe();
        // Write up to default capacity.
        let data = alloc::vec![b'x'; DEFAULT_PIPE_CAPACITY];
        let written = writer.write(&data).unwrap();
        assert_eq!(written, DEFAULT_PIPE_CAPACITY);
    }

    #[test]
    fn test_custom_capacity() {
        let (_reader, writer) = create_pipe_with_capacity(16);
        let data = alloc::vec![b'y'; 32];
        let written = writer.write(&data).unwrap();
        assert_eq!(written, 16);
    }

    #[test]
    fn test_writer_drop_signals_eof() {
        let (reader, writer) = create_pipe();
        assert!(!reader.is_eof());

        drop(writer);
        assert!(reader.is_eof());
    }

    #[test]
    fn test_reader_drop_signals_broken() {
        let (reader, writer) = create_pipe();
        assert!(!writer.is_broken());

        drop(reader);
        assert!(writer.is_broken());
    }

    #[test]
    fn test_eof_with_buffered_data() {
        let (reader, writer) = create_pipe();
        writer.write(b"data").unwrap();
        drop(writer);

        // EOF is false because there is still buffered data.
        assert!(!reader.is_eof());

        // Read all data.
        let mut buf = [0u8; 16];
        reader.read(&mut buf);

        // Now EOF should be true.
        assert!(reader.is_eof());
    }

    #[test]
    fn test_epipe_on_second_write_after_reader_drop() {
        let (reader, writer) = create_pipe();
        // Fill the pipe so first write would block/return 0.
        writer
            .write(&alloc::vec![b'x'; DEFAULT_PIPE_CAPACITY])
            .unwrap();
        drop(reader);

        // Even though the pipe is full, EPIPE takes priority.
        let result = writer.write(b"extra");
        assert_eq!(result, Err(EPIPE));
    }
}
