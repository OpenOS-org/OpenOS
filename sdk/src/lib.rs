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
    pub const EVENT_CREATE: u64 = 0xF2;
    pub const EVENT_SIGNAL: u64 = 0xF3;
    pub const EVENT_WAIT: u64 = 0xFB;
    pub const EVENT_DESTROY: u64 = 0xFC;
    pub const FS_OPEN: u64 = 0xF7;
    pub const FS_READ: u64 = 0xF8;
    pub const FS_WRITE: u64 = 0xF9;
    pub const FS_CLOSE: u64 = 0xFA;
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
        let raw =
            unsafe { raw::syscall2(number::CONSOLE_WRITE, msg.as_ptr() as u64, msg.len() as u64) };
        result(raw).map(|v| v as usize)
    }

    /// Write a message followed by a newline.
    pub fn writeln(msg: &str) -> Result<usize, Error> {
        write(msg)?;
        write("\n")
    }
}

/// Event signaling operations.
///
/// Events are kernel objects that can be signaled by one task and waited on
/// by another. An event is initially unsignaled. Calling `signal` transitions
/// it to the signaled state. Calling `wait` blocks until the event is signaled,
/// then clears the signal (level-triggered semantics).
pub mod event {
    use super::*;

    /// Create a new unsignaled event. Returns a handle.
    pub fn create() -> Result<Handle, Error> {
        let raw = unsafe { raw::syscall0(number::EVENT_CREATE) };
        result(raw).map(Handle::from_raw)
    }

    /// Signal an event, waking any task blocked in `wait`.
    pub fn signal(handle: Handle) -> Result<(), Error> {
        let raw = unsafe { raw::syscall1(number::EVENT_SIGNAL, handle.as_raw()) };
        result(raw)?;
        Ok(())
    }

    /// Block until the event is signaled, then clear the signal.
    pub fn wait(handle: Handle) -> Result<(), Error> {
        let raw = unsafe { raw::syscall1(number::EVENT_WAIT, handle.as_raw()) };
        result(raw)?;
        Ok(())
    }

    /// Destroy an event by closing its handle.
    pub fn destroy(handle: Handle) -> Result<(), Error> {
        let raw = unsafe { raw::syscall1(number::EVENT_DESTROY, handle.as_raw()) };
        result(raw)?;
        Ok(())
    }
}

/// Filesystem operations via kernel syscalls.
///
/// Provides direct access to the kernel's ramfs through system calls.
/// File descriptors 0 and 1 are reserved for stdin and stdout.
pub mod fs {
    use super::*;

    /// Reserved file descriptor for standard input.
    pub const FD_STDIN: u64 = 0;

    /// Reserved file descriptor for standard output.
    pub const FD_STDOUT: u64 = 1;

    /// Open a file by name. Returns a file descriptor.
    ///
    /// The file must already exist in ramfs.
    pub fn open(name: &str) -> Result<u64, Error> {
        let raw =
            unsafe { raw::syscall3(number::FS_OPEN, name.as_ptr() as u64, name.len() as u64, 0) };
        result(raw)
    }

    /// Read bytes from an open file descriptor into `buf`.
    ///
    /// Returns the number of bytes read (0 at EOF).
    pub fn read(fd: u64, buf: &mut [u8]) -> Result<usize, Error> {
        let raw = unsafe {
            raw::syscall3(
                number::FS_READ,
                fd,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
            )
        };
        result(raw).map(|v| v as usize)
    }

    /// Write bytes to an open file descriptor.
    ///
    /// Writing to `FD_STDOUT` redirects to the serial console.
    /// Returns the number of bytes written.
    pub fn write(fd: u64, data: &[u8]) -> Result<usize, Error> {
        let raw = unsafe {
            raw::syscall3(
                number::FS_WRITE,
                fd,
                data.as_ptr() as u64,
                data.len() as u64,
            )
        };
        result(raw).map(|v| v as usize)
    }

    /// Close a file descriptor.
    pub fn close(fd: u64) -> Result<(), Error> {
        let raw = unsafe { raw::syscall1(number::FS_CLOSE, fd) };
        result(raw)?;
        Ok(())
    }
}
