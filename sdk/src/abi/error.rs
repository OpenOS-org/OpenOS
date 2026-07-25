//! Error types matching the kernel's syscall return convention.
//!
//! The kernel returns errors as `error_code | (1 << 63)`. The high bit
//! acts as an error flag. This module provides types to decode and
//! represent these errors ergonomically.

/// Bit 63 is set on error returns from the kernel.
const ERROR_FLAG: u64 = 1 << 63;

/// Error codes returned by the kernel's syscall handler.
///
/// These match the kernel's `SyscallError` enum values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub enum Error {
    /// Unrecognized syscall number.
    InvalidSyscall = 1,
    /// Argument validation failed (null pointer, zero length, etc.).
    InvalidArgument = 2,
    /// Caller lacks the required capability.
    PermissionDenied = 3,
}

impl Error {
    /// Create an `Error` from a raw error code (lower bits of the return value).
    fn from_code(code: u64) -> Self {
        match code {
            2 => Self::InvalidArgument,
            3 => Self::PermissionDenied,
            _ => Self::InvalidSyscall,
        }
    }
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidSyscall => write!(f, "invalid syscall number"),
            Self::InvalidArgument => write!(f, "invalid argument"),
            Self::PermissionDenied => write!(f, "permission denied"),
        }
    }
}

/// Result type for syscall operations.
pub type Result<T> = core::result::Result<T, Error>;

/// Decode a raw syscall return value into a `Result`.
///
/// # Errors
///
/// Returns the decoded `Error` if bit 63 is set in the return value.
pub fn decode(value: u64) -> Result<u64> {
    if value & ERROR_FLAG != 0 {
        Err(Error::from_code(value & !ERROR_FLAG))
    } else {
        Ok(value)
    }
}
