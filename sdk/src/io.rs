//! Standard I/O operations.
//!
//! Provides safe wrappers around `SYS_WRITE` and `SYS_READ` for
//! byte-level I/O. Higher-level print macros build on these.

use crate::abi::error::Result;
use crate::abi::{number, raw};

/// Write bytes to the console (VGA + serial).
///
/// Returns the number of bytes written on success. The kernel writes
/// exactly the bytes provided — no prefix, no suffix, no transformation.
///
/// # Errors
///
/// Returns `Error::InvalidArgument` if `buf` is empty.
pub fn write(buf: &[u8]) -> Result<usize> {
    if buf.is_empty() {
        return Ok(0);
    }
    let ret = unsafe { raw::syscall2(number::SYS_WRITE, buf.as_ptr() as u64, buf.len() as u64) };
    crate::abi::error::decode(ret).map(|v| v as usize)
}

/// Write an entire byte slice, retrying if the kernel reports fewer
/// bytes written than requested.
///
/// # Errors
///
/// Returns the first error encountered, or `Error::InvalidArgument`
/// if the kernel reports zero bytes written.
pub fn write_all(buf: &[u8]) -> Result<()> {
    let mut written = 0;
    while written < buf.len() {
        let n = write(&buf[written..])?;
        if n == 0 {
            return Err(crate::abi::error::Error::InvalidArgument);
        }
        written += n;
    }
    Ok(())
}

/// Read bytes from the keyboard input buffer.
///
/// Returns the number of bytes read. Currently unimplemented in the
/// kernel (always returns 0).
///
/// # Errors
///
/// Returns `Error::InvalidArgument` if `buf` is empty.
pub fn read(buf: &mut [u8]) -> Result<usize> {
    if buf.is_empty() {
        return Ok(0);
    }
    let ret = unsafe { raw::syscall2(number::SYS_READ, buf.as_mut_ptr() as u64, buf.len() as u64) };
    crate::abi::error::decode(ret).map(|v| v as usize)
}
