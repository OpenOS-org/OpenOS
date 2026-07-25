//! System call numbers.
//!
//! This file mirrors `kernel/src/syscall/number.rs` to ensure ABI
//! compatibility. Both kernel and SDK must use the same numbers.
//!
//! **When adding a new syscall:**
//!   1. Add the constant here AND in `kernel/src/syscall/number.rs`
//!   2. Add the handler in `kernel/src/syscall/mod.rs`
//!   3. Update `docs/API.md` syscall table
//!   4. Add a safe wrapper in the SDK

/// Write bytes to the console (VGA + serial).
///
/// - `arg1`: `*const u8` — pointer to byte buffer
/// - `arg2`: `u64` — number of bytes to write
/// - Returns: bytes written, or error
pub const SYS_WRITE: u64 = 1;

/// Read bytes from the keyboard input buffer.
///
/// - `arg1`: `*mut u8` — pointer to destination buffer
/// - `arg2`: `u64` — maximum bytes to read
/// - Returns: bytes read, or error
pub const SYS_READ: u64 = 2;

/// Terminate the calling process.
///
/// - `arg1`: `u64` — exit code (0 = success)
/// - Returns: does not return
pub const SYS_EXIT: u64 = 3;

/// Yield the CPU to the next task.
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
/// - `arg2`: `*const Message` — pointer to message
/// - Returns: `0`, or error
pub const SYS_SEND: u64 = 6;

/// Receive an IPC message from a port (blocking).
///
/// - `arg1`: `u64` — source port ID
/// - `arg2`: `*mut Message` — pointer to receive buffer
/// - Returns: `0`, or error
pub const SYS_RECEIVE: u64 = 7;
