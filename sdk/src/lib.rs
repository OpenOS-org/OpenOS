//! OpenOS User-Space SDK
//!
//! Safe Rust wrappers around the OpenOS system call interface.
//! User programs depend on this crate to interact with the kernel.
//!
//! # Example
//! ```no_run
//! use openos_sdk::{channel, process};
//!
//! let (handle_a, handle_b) = channel::create().unwrap();
//! channel::send(handle_a, b"hello").unwrap();
//! process::exit(0);
//! ```

#![no_std]

/// Raw system call interface.
///
/// These functions invoke the `syscall` instruction directly.
/// Prefer the safe wrappers in `channel`, `handle`, `process`, and `console`.
pub mod raw {
    /// Invoke a system call with 0-5 arguments.
    ///
    /// Returns the raw i64 result. Positive = success, negative = error.
    pub unsafe fn syscall0(number: u64) -> i64 {
        let result: i64;
        core::arch::asm!(
            "syscall",
            in("rax") number,
            lateout("rax") result,
            out("rcx") _,
            out("r11") _,
        );
        result
    }

    pub unsafe fn syscall1(number: u64, arg1: u64) -> i64 {
        let result: i64;
        core::arch::asm!(
            "syscall",
            in("rax") number,
            in("rdi") arg1,
            lateout("rax") result,
            out("rcx") _,
            out("r11") _,
        );
        result
    }

    pub unsafe fn syscall2(number: u64, arg1: u64, arg2: u64) -> i64 {
        let result: i64;
        core::arch::asm!(
            "syscall",
            in("rax") number,
            in("rdi") arg1,
            in("rsi") arg2,
            lateout("rax") result,
            out("rcx") _,
            out("r11") _,
        );
        result
    }

    pub unsafe fn syscall3(number: u64, arg1: u64, arg2: u64, arg3: u64) -> i64 {
        let result: i64;
        core::arch::asm!(
            "syscall",
            in("rax") number,
            in("rdi") arg1,
            in("rsi") arg2,
            in("rdx") arg3,
            lateout("rax") result,
            out("rcx") _,
            out("r11") _,
        );
        result
    }
}

/// System call numbers (must match kernel/src/syscall/number.rs).
mod number {
    pub const CHANNEL_CREATE: u64 = 0x01;
    pub const CHANNEL_SEND: u64 = 0x02;
    pub const CHANNEL_RECEIVE: u64 = 0x03;
    pub const CHANNEL_CALL: u64 = 0x04;
    pub const CHANNEL_REPLY: u64 = 0x05;
    pub const HANDLE_CLOSE: u64 = 0x10;
    pub const HANDLE_DUPLICATE: u64 = 0x11;
    pub const HANDLE_TRANSFER: u64 = 0x12;
    pub const PROCESS_CREATE: u64 = 0x30;
    pub const PROCESS_START: u64 = 0x31;
    pub const PROCESS_EXIT: u64 = 0x32;
    pub const PROCESS_WAIT: u64 = 0x33;
    pub const THREAD_CREATE: u64 = 0x40;
    pub const THREAD_EXIT: u64 = 0x41;
    pub const THREAD_YIELD: u64 = 0x42;
    pub const CONSOLE_WRITE: u64 = 0xF0;
}

/// Error type for system calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Invalid argument.
    InvalidArgument,
    /// Resource not found.
    NotFound,
    /// Permission denied.
    PermissionDenied,
    /// Out of memory.
    OutOfMemory,
    /// Resource busy.
    Busy,
    /// Channel closed.
    ChannelClosed,
    /// Operation would block.
    WouldBlock,
    /// Operation timed out.
    Timeout,
    /// Bad pointer (null or kernel-space).
    BadPointer,
    /// Unknown syscall number.
    UnknownSyscall,
    /// Unknown error code.
    Unknown(i64),
}

impl Error {
    /// Convert a raw error code to an `Error`.
    fn from_raw(code: i64) -> Self {
        match code {
            -1 => Self::InvalidArgument,
            -2 => Self::NotFound,
            -3 => Self::PermissionDenied,
            -4 => Self::OutOfMemory,
            -5 => Self::Busy,
            -6 => Self::ChannelClosed,
            -7 => Self::WouldBlock,
            -8 => Self::Timeout,
            -9 => Self::BadPointer,
            -10 => Self::UnknownSyscall,
            n => Self::Unknown(n),
        }
    }
}

/// Convert a raw syscall result to `Result<u64, Error>`.
fn result(raw: i64) -> Result<u64, Error> {
    if raw >= 0 {
        Ok(raw as u64)
    } else {
        Err(Error::from_raw(raw))
    }
}

/// A handle to a kernel object.
///
/// Handles are opaque tokens that reference kernel objects (channels, memory,
/// processes). They are the only way user-space interacts with the kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handle(u64);

impl Handle {
    /// Create a handle from a raw u64 value.
    pub fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// Get the raw u64 value.
    pub fn as_raw(&self) -> u64 {
        self.0
    }
}

/// Channel operations.
pub mod channel {
    use super::*;

    /// Create a new channel. Returns two handles (end_a, end_b).
    pub fn create() -> Result<(Handle, Handle), Error> {
        let raw = unsafe { raw::syscall0(number::CHANNEL_CREATE) };
        let id = result(raw)?;
        // For now, the kernel returns a single channel ID.
        // Both handles reference the same channel with different ends.
        Ok((Handle::from_raw(id), Handle::from_raw(id | 0x100000000)))
    }

    /// Send a message on a channel handle.
    pub fn send(handle: Handle, msg: &[u8]) -> Result<(), Error> {
        let raw = unsafe {
            raw::syscall3(
                number::CHANNEL_SEND,
                handle.as_raw(),
                msg.as_ptr() as u64,
                msg.len() as u64,
            )
        };
        result(raw)?;
        Ok(())
    }

    /// Receive a message on a channel handle.
    /// Blocks until a message is available.
    pub fn receive(handle: Handle, buf: &mut [u8]) -> Result<usize, Error> {
        let raw = unsafe {
            raw::syscall3(
                number::CHANNEL_RECEIVE,
                handle.as_raw(),
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
            )
        };
        result(raw).map(|v| v as usize)
    }

    /// Call = send + block for reply (atomic RPC).
    pub fn call(handle: Handle, msg: &[u8], reply_buf: &mut [u8]) -> Result<usize, Error> {
        let raw = unsafe {
            raw::syscall3(
                number::CHANNEL_CALL,
                handle.as_raw(),
                msg.as_ptr() as u64,
                reply_buf.as_mut_ptr() as u64,
            )
        };
        result(raw).map(|v| v as usize)
    }

    /// Reply to a received message.
    pub fn reply(handle: Handle, msg: &[u8]) -> Result<(), Error> {
        let raw = unsafe {
            raw::syscall3(
                number::CHANNEL_REPLY,
                handle.as_raw(),
                msg.as_ptr() as u64,
                msg.len() as u64,
            )
        };
        result(raw)?;
        Ok(())
    }
}

/// Process operations.
pub mod process {
    use super::*;

    /// Exit the current process with the given status code.
    /// This function does not return.
    pub fn exit(status: u64) -> ! {
        unsafe {
            raw::syscall1(number::PROCESS_EXIT, status);
        }
        unreachable!()
    }
}

/// Console operations (debug output).
pub mod console {
    use super::*;

    /// Write a message to the kernel's debug console (serial port).
    pub fn write(msg: &str) -> Result<usize, Error> {
        let raw = unsafe {
            raw::syscall2(
                number::CONSOLE_WRITE,
                msg.as_ptr() as u64,
                msg.len() as u64,
            )
        };
        result(raw).map(|v| v as usize)
    }

    /// Write a message followed by a newline.
    pub fn writeln(msg: &str) -> Result<usize, Error> {
        write(msg)?;
        write("\n")
    }
}

/// Filesystem operations via Channel IPC.
///
/// File operations are messages sent to a filesystem server process.
/// The server runs in user-space and manages an in-memory filesystem.
///
/// # Protocol
///
/// All messages are serialized as:
///   [opcode: u8] [args...]
///
/// Responses are:
///   [status: u8] [data...]
///
/// Opcodes:
///   0x01 = Open   → response: [status, fd: u32]
///   0x02 = Read   → response: [status, data...]
///   0x03 = Write  → response: [status, bytes_written: u32]
///   0x04 = Close  → response: [status]
///   0x05 = Create → response: [status]
pub mod fs {
    use super::*;

    /// File operation opcodes.
    pub mod opcode {
        pub const OPEN: u8 = 0x01;
        pub const READ: u8 = 0x02;
        pub const WRITE: u8 = 0x03;
        pub const CLOSE: u8 = 0x04;
        pub const CREATE: u8 = 0x05;
    }

    /// File open flags.
    pub mod flags {
        pub const READ: u8 = 0x01;
        pub const WRITE: u8 = 0x02;
        pub const CREATE: u8 = 0x04;
    }

    /// Maximum filename length.
    pub const MAX_NAME_LEN: usize = 255;

    /// Maximum data per read/write operation.
    pub const MAX_DATA_LEN: usize = 4096;

    /// Build an OPEN message: [opcode, flags, name...]
    pub fn msg_open(name: &str, flags: u8) -> Result<[u8; 260], Error> {
        if name.len() > MAX_NAME_LEN {
            return Err(Error::InvalidArgument);
        }
        let mut buf = [0u8; 260];
        buf[0] = opcode::OPEN;
        buf[1] = flags;
        buf[2..2 + name.len()].copy_from_slice(name.as_bytes());
        Ok(buf)
    }

    /// Build a READ message: [opcode, fd, max_len]
    pub fn msg_read(fd: u32, max_len: u32) -> [u8; 9] {
        let mut buf = [0u8; 9];
        buf[0] = opcode::READ;
        buf[1..5].copy_from_slice(&fd.to_le_bytes());
        buf[5..9].copy_from_slice(&max_len.to_le_bytes());
        buf
    }

    /// Build a WRITE message into a caller-provided buffer.
    /// Returns the number of bytes written to `buf`.
    pub fn build_write_msg(fd: u32, data: &[u8], buf: &mut [u8]) -> Result<usize, Error> {
        if data.len() > MAX_DATA_LEN || 5 + data.len() > buf.len() {
            return Err(Error::InvalidArgument);
        }
        buf[0] = opcode::WRITE;
        buf[1..5].copy_from_slice(&fd.to_le_bytes());
        buf[5..5 + data.len()].copy_from_slice(data);
        Ok(5 + data.len())
    }

    /// Build a CLOSE message: [opcode, fd]
    pub fn msg_close(fd: u32) -> [u8; 5] {
        let mut buf = [0u8; 5];
        buf[0] = opcode::CLOSE;
        buf[1..5].copy_from_slice(&fd.to_le_bytes());
        buf
    }

    /// Parse an OPEN response: [status, fd: u32]
    pub fn parse_open_response(data: &[u8]) -> Result<u32, Error> {
        if data.len() < 5 || data[0] != 0 {
            return Err(Error::NotFound);
        }
        Ok(u32::from_le_bytes([data[1], data[2], data[3], data[4]]))
    }

    /// Parse a READ response: [status, data...]
    pub fn parse_read_response(data: &[u8]) -> Result<&[u8], Error> {
        if data.is_empty() || data[0] != 0 {
            return Err(Error::NotFound);
        }
        Ok(&data[1..])
    }
}

