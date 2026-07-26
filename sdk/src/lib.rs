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
#[allow(dead_code)]
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
    pub const CONSOLE_READ: u64 = 0xF4;
    pub const EVENT_CREATE: u64 = 0xF2;
    pub const EVENT_SIGNAL: u64 = 0xF3;
    pub const EVENT_WAIT: u64 = 0xFB;
    pub const EVENT_DESTROY: u64 = 0xFC;
    pub const FS_OPEN: u64 = 0xF7;
    pub const FS_READ: u64 = 0xF8;
    pub const FS_WRITE: u64 = 0xF9;
    pub const FS_CLOSE: u64 = 0xFA;
    pub const FS_SEEK: u64 = 0xFF;
    pub const NET_SEND: u64 = 0xFD;
    pub const NET_RECEIVE: u64 = 0xFE;
    pub const SOCKET: u64 = 0xA0;
    pub const CONNECT: u64 = 0xA4;
    pub const SENDTO: u64 = 0xA5;
    pub const RECVFROM: u64 = 0xA6;
    pub const CLOSE_SOCK: u64 = 0xA7;
    pub const DNS_RESOLVE: u64 = 0xA8;
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

    /// Create a new process. Returns the task ID.
    ///
    /// The process is created in the scheduler but not yet running.
    /// Call `start` to load an ELF and begin execution.
    pub fn create(name: &str) -> Result<u64, Error> {
        let raw = unsafe {
            raw::syscall3(
                number::PROCESS_CREATE,
                0, // job handle (unused)
                name.as_ptr() as u64,
                name.len() as u64,
            )
        };
        result(raw)
    }

    /// Start a previously created process by loading an ELF from the initrd.
    ///
    /// The `task_id` should come from `create`. The `elf_filename` is the
    /// name of the ELF binary in the initrd archive.
    pub fn start(task_id: u64, elf_filename: &str) -> Result<u64, Error> {
        let raw = unsafe {
            raw::syscall3(
                number::PROCESS_START,
                task_id,
                elf_filename.as_ptr() as u64,
                elf_filename.len() as u64,
            )
        };
        result(raw)
    }

    /// Exit the current process with the given status code.
    /// This function does not return.
    pub fn exit(status: u64) -> ! {
        unsafe {
            raw::syscall1(number::PROCESS_EXIT, status);
        }
        unreachable!()
    }

    /// Wait for a child process to exit.
    ///
    /// `task_id` is the child's task ID (from `create`).
    /// `timeout_ticks` is the maximum number of timer ticks to wait.
    /// Pass `u64::MAX` to block indefinitely.
    ///
    /// Returns the child's exit status on success.
    pub fn wait(task_id: u64, timeout_ticks: u64) -> Result<u64, Error> {
        let raw = unsafe { raw::syscall2(number::PROCESS_WAIT, task_id, timeout_ticks) };
        result(raw)
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

    /// Read characters from the kernel's debug console (keyboard input).
    ///
    /// If `blocking` is true, blocks until at least one character is available.
    /// Returns the number of bytes read.
    pub fn read(buf: &mut [u8], blocking: bool) -> Result<usize, Error> {
        let flags: u64 = if blocking { 1 } else { 0 };
        let raw = unsafe {
            raw::syscall3(
                number::CONSOLE_READ,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
                flags,
            )
        };
        result(raw).map(|v| v as usize)
    }
}

/// Handle operations.
///
/// Handles are opaque tokens that reference kernel objects. These functions
/// allow closing, duplicating, and transferring handles between tasks.
pub mod handle {
    use super::*;

    /// Close a handle, releasing the kernel object it references.
    pub fn close(handle: Handle) -> Result<(), Error> {
        let raw = unsafe { raw::syscall1(number::HANDLE_CLOSE, handle.as_raw()) };
        result(raw)?;
        Ok(())
    }

    /// Duplicate a handle with optionally narrowed rights.
    ///
    /// Returns a new handle that references the same kernel object.
    pub fn duplicate(handle: Handle, new_rights: u16) -> Result<Handle, Error> {
        let raw =
            unsafe { raw::syscall2(number::HANDLE_DUPLICATE, handle.as_raw(), new_rights as u64) };
        result(raw).map(Handle::from_raw)
    }

    /// Transfer a handle through a channel to another task.
    ///
    /// The handle is removed from the sender's handle table and attached
    /// to the next message sent on the channel. The receiver gets it
    /// as part of the message.
    pub fn transfer(handle: Handle, channel: Handle) -> Result<(), Error> {
        let raw =
            unsafe { raw::syscall2(number::HANDLE_TRANSFER, handle.as_raw(), channel.as_raw()) };
        result(raw)?;
        Ok(())
    }
}

/// Thread operations.
pub mod thread {
    use super::*;

    /// Yield the current thread's remaining time slice to the scheduler.
    pub fn yield_() {
        unsafe {
            raw::syscall0(number::THREAD_YIELD);
        }
    }

    /// Exit the current thread.
    /// This function does not return.
    pub fn exit() -> ! {
        unsafe {
            raw::syscall0(number::THREAD_EXIT);
        }
        unreachable!()
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

/// Network operations via kernel syscalls.
///
/// Provides raw Ethernet frame send/receive through the virtio-net driver.
/// The TCP/IP stack runs in user-space on top of these primitives.
pub mod net {
    use super::*;

    /// Send a raw Ethernet frame.
    pub fn send_frame(data: &[u8]) -> Result<usize, Error> {
        let raw =
            unsafe { raw::syscall2(number::NET_SEND, data.as_ptr() as u64, data.len() as u64) };
        result(raw).map(|v| v as usize)
    }

    /// Receive a raw Ethernet frame (non-blocking).
    /// Returns the frame data if available, or `WouldBlock` error.
    pub fn receive_frame(buf: &mut [u8]) -> Result<usize, Error> {
        let raw = unsafe {
            raw::syscall2(
                number::NET_RECEIVE,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
            )
        };
        result(raw).map(|v| v as usize)
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

    /// Seek to a position in an open file descriptor.
    ///
    /// `whence` values:
    ///   - 0: SEEK_SET (from beginning of file)
    ///   - 1: SEEK_CUR (from current position)
    ///   - 2: SEEK_END (from end of file)
    ///
    /// Returns the new absolute offset.
    pub fn seek(fd: u64, offset: i64, whence: u64) -> Result<u64, Error> {
        let raw = unsafe { raw::syscall3(number::FS_SEEK, fd, offset as u64, whence) };
        result(raw)
    }

    /// Close a file descriptor.
    pub fn close(fd: u64) -> Result<(), Error> {
        let raw = unsafe { raw::syscall1(number::FS_CLOSE, fd) };
        result(raw)?;
        Ok(())
    }
}

/// Socket operations via kernel syscalls.
///
/// Provides TCP socket connect, send, receive, and close through system calls.
pub mod socket {
    use super::*;

    /// Create a TCP socket. Returns a socket descriptor.
    pub fn create_tcp() -> Result<u64, Error> {
        // 0 = Tcp socket type.
        let raw = unsafe { raw::syscall1(number::SOCKET, 0) };
        result(raw)
    }

    /// Connect a TCP socket to a remote address and port.
    ///
    /// `addr` is the IPv4 address in network byte order (4 bytes, as u64).
    /// `port` is the remote port number.
    pub fn connect(sock_fd: u64, addr: u32, port: u16) -> Result<(), Error> {
        let raw = unsafe { raw::syscall3(number::CONNECT, sock_fd, addr as u64, port as u64) };
        result(raw)?;
        Ok(())
    }

    /// Send data on a connected TCP socket.
    ///
    /// Returns the number of bytes sent.
    pub fn send(sock_fd: u64, data: &[u8]) -> Result<usize, Error> {
        let raw = unsafe {
            raw::syscall3(
                number::SENDTO,
                sock_fd,
                data.as_ptr() as u64,
                data.len() as u64,
            )
        };
        result(raw).map(|v| v as usize)
    }

    /// Receive data from a connected TCP socket (non-blocking).
    ///
    /// Returns the number of bytes received, or `WouldBlock` if no data is
    /// available.
    pub fn recv(sock_fd: u64, buf: &mut [u8]) -> Result<usize, Error> {
        let raw = unsafe {
            raw::syscall3(
                number::RECVFROM,
                sock_fd,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
            )
        };
        result(raw).map(|v| v as usize)
    }

    /// Close a socket.
    pub fn close(sock_fd: u64) -> Result<(), Error> {
        let raw = unsafe { raw::syscall1(number::CLOSE_SOCK, sock_fd) };
        result(raw)?;
        Ok(())
    }
}

/// DNS resolution via kernel syscall.
///
/// Resolves a hostname to an IPv4 address using the kernel's DNS resolver.
pub mod dns {
    use super::*;

    /// Resolve a hostname to a 4-byte IPv4 address.
    ///
    /// The returned address is in network byte order.
    pub fn resolve(hostname: &str) -> Result<[u8; 4], Error> {
        let mut ip = [0u8; 4];
        let raw = unsafe {
            raw::syscall3(
                number::DNS_RESOLVE,
                hostname.as_ptr() as u64,
                hostname.len() as u64,
                ip.as_mut_ptr() as u64,
            )
        };
        result(raw)?;
        Ok(ip)
    }
}
