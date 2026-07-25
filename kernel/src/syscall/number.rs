//! System call numbers.
//!
//! This module is the single source of truth for all syscall numbers.
//! Both kernel and user-space must include this file to ensure ABI
//! compatibility. Never use raw numbers — always reference these constants.
//!
//! To add a new syscall:
//!   1. Add the constant here
//!   2. Add the handler in `mod.rs`
//!   3. Update `docs/API.md` syscall table

/// Write bytes to the VGA console.
///
/// - `arg1`: `*const u8` — pointer to byte buffer in user-space
/// - `arg2`: `u64` — number of bytes to write
/// - Returns: bytes written, or error
pub const SYS_WRITE: u64 = 1;

/// Read bytes from the keyboard input buffer.
///
/// - `arg1`: `*mut u8` — pointer to destination buffer in user-space
/// - `arg2`: `u64` — maximum bytes to read
/// - Returns: bytes read, or error
pub const SYS_READ: u64 = 2;

/// Terminate the calling process.
///
/// - `arg1`: `u64` — exit code (0 = success)
/// - Returns: does not return
pub const SYS_EXIT: u64 = 3;

/// Yield the CPU to the next task in the scheduler.
///
/// - Returns: `0` (always succeeds)
pub const SYS_YIELD: u64 = 4;

/// Allocate a new IPC port.
///
/// - Returns: port ID, or error
pub const SYS_PORT_CREATE: u64 = 5;

/// Send an IPC message to a port.
///
/// - `arg1`: `u64` — target port ID
/// - `arg2`: `*const Message` — pointer to message in user-space
/// - Returns: `0`, or error
pub const SYS_SEND: u64 = 6;

/// Receive an IPC message from a port (blocking).
///
/// - `arg1`: `u64` — source port ID
/// - `arg2`: `*mut Message` — pointer to receive buffer in user-space
/// - Returns: `0`, or error
pub const SYS_RECEIVE: u64 = 7;

/// Total number of syscalls (for bounds checking).
pub const SYS_COUNT: u64 = 8;
