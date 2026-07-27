//! OpenOS User-Space SDK
//!
//! Safe Rust wrappers around the OpenOS system call interface. User-space
//! programs depend on this crate to interact with the kernel for IPC, process
//! management, file I/O, networking, and device access.
//!
//! # Modules
//!
//! | Module | Purpose |
//! |--------|---------|
//! | [`channel`] | IPC channel message passing (send, receive, call/reply) |
//! | [`handle`] | Capability handle lifecycle (close, duplicate, transfer) |
//! | [`process`] | Process creation, lifecycle, and task listing |
//! | [`thread`] | Thread creation, yield, and exit |
//! | [`console`] | Debug console read/write (serial port) |
//! | [`fs`] | Filesystem operations (open, read, write, seek, stat, pipe) |
//! | [`socket`] | TCP socket operations (connect, bind, listen, accept) |
//! | [`dns`] | DNS hostname resolution |
//! | [`net`] | Raw Ethernet frame send/receive |
//! | [`memory`] | Virtual memory management (brk, mmap, munmap, mprotect) |
//! | [`event`] | Cross-task event signaling |
//! | [`signal`] | Process signal delivery and handler installation |
//! | [`service`] | Named service endpoint registration and discovery |
//! | [`env`] | Environment variables and working directory |
//! | [`time`] | Monotonic clock and sleep |
//! | [`device`] | Port I/O, MMIO, and IRQ access for user-space drivers |
//! | [`io`] | Scatter-gather I/O (readv/writev) |
//! | [`resource`] | Resource usage and limits (getrusage/prlimit) |
//! | [`shm`] | Shared memory segments (shmget/shmat/shmdt) |
//! | [`misc`] | Miscellaneous syscalls (ioctl, getrandom, epoll, timers) |
//!
//! # Error Handling
//!
//! All fallible functions return `Result<T, Error>`. The [`Error`] enum
//! maps raw kernel error codes to descriptive variants.
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

extern crate alloc;

/// Raw system call interface.
///
/// These functions invoke the `syscall` instruction directly, passing
/// arguments in the x86_64 SysV convention registers (rdi, rsi, rdx,
/// r10, r8). The return value in rax is interpreted as `i64` where
/// positive values indicate success and negative values indicate errors.
///
/// # Safety
///
/// All functions in this module are `unsafe` because they bypass all
/// Rust safety guarantees — incorrect arguments can corrupt memory or
/// crash the process. Prefer the safe wrappers in other SDK modules.
pub mod raw {
    /// Invoke a system call with 0 arguments.
    ///
    /// Returns the raw i64 result. Positive = success, negative = error.
    ///
    /// # Safety
    ///
    /// The syscall number must be valid and the syscall must not require any arguments.
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

    /// Invoke a system call with 1 argument.
    ///
    /// # Safety
    ///
    /// `number` must be a valid syscall and `arg1` must be valid for that syscall.
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

    /// Invoke a system call with 2 arguments.
    ///
    /// # Safety
    ///
    /// All arguments must be valid for the given syscall number.
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

    /// Invoke a system call with 3 arguments.
    ///
    /// # Safety
    ///
    /// All arguments must be valid for the given syscall number.
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

    /// Invoke a system call with 4 arguments.
    ///
    /// # Safety
    ///
    /// All arguments must be valid for the given syscall number.
    pub unsafe fn syscall4(number: u64, arg1: u64, arg2: u64, arg3: u64, arg4: u64) -> i64 {
        let result: i64;
        core::arch::asm!(
            "syscall",
            in("rax") number,
            in("rdi") arg1,
            in("rsi") arg2,
            in("rdx") arg3,
            in("r10") arg4,
            lateout("rax") result,
            out("rcx") _,
            out("r11") _,
        );
        result
    }

    /// Invoke a system call with 5 arguments.
    ///
    /// # Safety
    ///
    /// All arguments must be valid for the given syscall number.
    pub unsafe fn syscall5(
        number: u64,
        arg1: u64,
        arg2: u64,
        arg3: u64,
        arg4: u64,
        arg5: u64,
    ) -> i64 {
        let result: i64;
        core::arch::asm!(
            "syscall",
            in("rax") number,
            in("rdi") arg1,
            in("rsi") arg2,
            in("rdx") arg3,
            in("r10") arg4,
            in("r8") arg5,
            lateout("rax") result,
            out("rcx") _,
            out("r11") _,
        );
        result
    }
}

/// System call numbers (must match kernel/src/syscall/number.rs).
///
/// These constants are the syscall identifiers passed in `rax` when
/// invoking the `syscall` instruction. They are internal to the SDK
/// and not exported.
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
    pub const BRK: u64 = 0x34;
    pub const MMAP: u64 = 0x35;
    pub const MUNMAP: u64 = 0x36;
    pub const MPROTECT: u64 = 0x4B;
    pub const GETPID: u64 = 0x37;
    pub const GETPPID: u64 = 0x38;
    pub const LIST_TASKS: u64 = 0x3D;
    pub const CLOCK_GETTIME: u64 = 0x3E;
    pub const THREAD_CREATE: u64 = 0x40;
    pub const THREAD_EXIT: u64 = 0x41;
    pub const THREAD_YIELD: u64 = 0x42;
    pub const KILL: u64 = 0x44;
    pub const SIGNAL: u64 = 0x45;
    pub const CONSOLE_WRITE: u64 = 0xF0;
    pub const SLEEP: u64 = 0xF1;
    pub const EVENT_CREATE: u64 = 0xF2;
    pub const CONSOLE_READ: u64 = 0xF4;
    pub const EVENT_SIGNAL: u64 = 0xF3;
    pub const EVENT_WAIT: u64 = 0xFB;
    pub const EVENT_DESTROY: u64 = 0xFC;
    pub const FS_OPEN: u64 = 0xF7;
    pub const FS_READ: u64 = 0xF8;
    pub const FS_WRITE: u64 = 0xF9;
    pub const FS_CLOSE: u64 = 0xFA;
    pub const FS_SEEK: u64 = 0xFF;
    pub const FS_UNLINK: u64 = 0xC0;
    pub const FS_RENAME: u64 = 0xC1;
    pub const FS_MKDIR: u64 = 0xC2;
    pub const FS_RMDIR: u64 = 0xC3;
    pub const FS_STAT: u64 = 0xC4;
    pub const FS_READDIR: u64 = 0xC5;
    pub const NET_SEND: u64 = 0xFD;
    pub const NET_RECEIVE: u64 = 0xFE;
    pub const SOCKET: u64 = 0xA0;
    pub const BIND: u64 = 0xA1;
    pub const LISTEN: u64 = 0xA2;
    pub const ACCEPT: u64 = 0xA3;
    pub const CONNECT: u64 = 0xA4;
    pub const SENDTO: u64 = 0xA5;
    pub const RECVFROM: u64 = 0xA6;
    pub const CLOSE_SOCK: u64 = 0xA7;
    pub const DNS_RESOLVE: u64 = 0xA8;
    pub const GETSOCKOPT: u64 = 0xA9;
    pub const SETSOCKOPT: u64 = 0xAA;
    pub const PORT_IN: u64 = 0xB0;
    pub const PORT_OUT: u64 = 0xB1;
    pub const MMIO_MAP: u64 = 0xB2;
    pub const MMIO_UNMAP: u64 = 0xB3;
    pub const IRQ_WAIT: u64 = 0xB4;
    pub const ENDPOINT_REGISTER: u64 = 0xF5;
    pub const ENDPOINT_DISCOVER: u64 = 0xF6;
    pub const FS_PIPE: u64 = 0x43;
    pub const DUP2: u64 = 0x47;
    pub const ENV_GET: u64 = 0x48;
    pub const ENV_SET: u64 = 0x49;
    pub const CHDIR: u64 = 0xCD;
    pub const GETCWD: u64 = 0xCE;
    pub const FS_FLOCK: u64 = 0x53;
    pub const FS_MKFIFO: u64 = 0xDC;
    pub const FSTAT: u64 = 0xC7;
    pub const LSTAT: u64 = 0xC8;
    pub const ACCESS: u64 = 0xC9;
    pub const SYMLINK: u64 = 0xCA;
    pub const READLINK: u64 = 0xCB;
    pub const CHMOD: u64 = 0xCC;
    pub const UMASK: u64 = 0xCF;
    pub const GETUID: u64 = 0xD5;
    pub const GETGID: u64 = 0xD6;
    pub const SETUID: u64 = 0xD7;
    pub const SETGID: u64 = 0xD8;
    pub const SHMGET: u64 = 0xD9;
    pub const SHMAT: u64 = 0xDA;
    pub const SHMDT: u64 = 0xDB;
    pub const GETPEERNAME: u64 = 0xAB;
    pub const GETSOCKNAME: u64 = 0xAC;
    pub const READV: u64 = 0xE0;
    pub const WRITEV: u64 = 0xE1;
    pub const IOCTL: u64 = 0xE6;
    pub const GETRUSAGE: u64 = 0xE7;
    pub const PRLIMIT: u64 = 0xE8;
    pub const GETTID: u64 = 0xE9;
    pub const GETRANDOM: u64 = 0xEA;
    pub const MEMBARRIER: u64 = 0xEB;
    pub const SCHED_YIELD: u64 = 0xEF;
    pub const TIMER_CREATE: u64 = 0xE3;
    pub const TIMER_SETTIME: u64 = 0xE4;
    pub const TIMER_GETTIME: u64 = 0xE5;
    pub const DUP3: u64 = 0x4E;
    pub const EPOLL_CREATE: u64 = 0x4F;
    pub const EPOLL_CTL: u64 = 0x50;
    pub const EPOLL_WAIT: u64 = 0x51;
    pub const MADVISE: u64 = 0x52;
    pub const GETDENTS64: u64 = 0xC6;
    pub const SYSLOG_DRAIN: u64 = 0xE2;
}

/// Error type returned by system calls.
///
/// Each variant maps to a negative kernel error code. Use pattern matching
/// or the `Debug` impl to inspect errors.
///
/// # Example
/// ```
/// use openos_sdk::Error;
///
/// let err = Error::from_raw(-2);
/// assert_eq!(err, Error::NotFound);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Argument out of range, null pointer, or otherwise invalid.
    InvalidArgument,
    /// Requested resource (file, handle, task) does not exist.
    NotFound,
    /// Caller lacks the required capability or rights.
    PermissionDenied,
    /// Kernel heap or frame allocator exhausted.
    OutOfMemory,
    /// Resource is in use and cannot be acquired (e.g., locked file).
    Busy,
    /// The IPC channel has been closed by the peer.
    ChannelClosed,
    /// Non-blocking operation has no data ready (try again later).
    WouldBlock,
    /// Blocking operation exceeded its deadline.
    Timeout,
    /// Pointer is null or points into kernel address space.
    BadPointer,
    /// Syscall number not recognized by the kernel.
    UnknownSyscall,
    /// Unrecognized error code (carries the raw negative value).
    Unknown(i64),
}

impl Error {
    /// Convert a raw negative kernel error code to an `Error` variant.
    ///
    /// Codes -1 through -10 map to specific variants; all others become
    /// `Unknown(code)`.
    pub fn from_raw(code: i64) -> Self {
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
///
/// Positive values are treated as success; negative values are mapped
/// to the corresponding [`Error`] variant.
fn result(raw: i64) -> Result<u64, Error> {
    if raw >= 0 {
        Ok(raw as u64)
    } else {
        Err(Error::from_raw(raw))
    }
}

/// A handle to a kernel object.
///
/// Handles are opaque tokens that reference kernel objects (channels, events,
/// IRQ events). They are the primary way user-space interacts with the kernel.
/// A handle encodes a slot ID, capability rights, and a generation counter
/// that prevents use-after-close exploitation.
///
/// Handles are closed automatically when dropped only if you implement
/// `Drop` yourself — the SDK does not auto-close. Call [`handle::close`]
/// explicitly when done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handle(u64);

impl Handle {
    /// Create a handle from a raw u64 value.
    ///
    /// This is used internally by the SDK to wrap syscall return values.
    /// User code rarely needs to call this directly.
    pub fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// Get the raw u64 value of this handle.
    ///
    /// Useful for passing handles to raw syscall wrappers.
    pub fn as_raw(&self) -> u64 {
        self.0
    }
}

/// File metadata (stat) information.
///
/// Returned by [`fs::stat`], [`fs::fstat`], and [`fs::lstat`]. The kernel
/// writes a 20-byte buffer: `size (u64) | ino (u64) | mode (u32)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stat {
    /// File size in bytes.
    pub size: u64,
    /// Inode number.
    pub ino: u64,
    /// File mode (permissions plus file type bits).
    pub mode: u32,
}

/// A scatter-gather I/O vector, matching the kernel's `iovec` layout
/// (16 bytes: pointer + length).
#[derive(Debug, Clone, Copy)]
pub struct IoVec {
    /// Pointer to the buffer.
    pub base: *const u8,
    /// Length of the buffer in bytes.
    pub len: usize,
}

/// Standard access mode constants for [`fs::access`].
pub mod access {
    /// Test for existence only.
    pub const F_OK: u64 = 0;
    /// Test for read permission.
    pub const R_OK: u64 = 1;
    /// Test for write permission.
    pub const W_OK: u64 = 2;
    /// Test for execute permission.
    pub const X_OK: u64 = 4;
}

/// IPC channel operations.
///
/// Channels are the primary inter-process communication primitive in OpenOS.
/// A channel has two ends; messages sent on one end can be received on the
/// other. Channels also support atomic call/reply (RPC) and handle transfer.
pub mod channel {
    use super::*;

    /// Create a new channel and return both ends as `(end_a, end_b)`.
    ///
    /// Either end can send or receive. Transfer one end to another task
    /// via [`handle::transfer`] to establish IPC.
    pub fn create() -> Result<(Handle, Handle), Error> {
        let raw = unsafe { raw::syscall0(number::CHANNEL_CREATE) };
        let id = result(raw)?;
        // For now, the kernel returns a single channel ID.
        // Both handles reference the same channel with different ends.
        Ok((Handle::from_raw(id), Handle::from_raw(id | 0x100000000)))
    }

    /// Send a message on a channel handle.
    ///
    /// The message is copied into the kernel. The peer can receive it
    /// by calling [`receive`] on the other end of the channel.
    ///
    /// # Errors
    ///
    /// - `ChannelClosed` — the peer has closed its end.
    /// - `InvalidArgument` — `handle` is not a valid channel end.
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
    ///
    /// Blocks the calling thread until a message is available. The message
    /// is copied into `buf`. If the message is larger than `buf`, it is
    /// truncated and the number of bytes written is returned.
    ///
    /// # Returns
    ///
    /// The number of bytes written to `buf`.
    ///
    /// # Errors
    ///
    /// - `ChannelClosed` — the sender has closed its end.
    /// - `InvalidArgument` — `handle` is not a valid channel end.
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
    ///
    /// Sends `msg` on the channel and blocks until the peer replies.
    /// The reply data is written into `reply_buf`.
    ///
    /// Returns the number of reply bytes written on success.
    pub fn call(handle: Handle, msg: &[u8], reply_buf: &mut [u8]) -> Result<usize, Error> {
        let raw = unsafe {
            raw::syscall5(
                number::CHANNEL_CALL,
                handle.as_raw(),
                msg.as_ptr() as u64,
                msg.len() as u64,
                reply_buf.as_mut_ptr() as u64,
                reply_buf.len() as u64,
            )
        };
        result(raw).map(|v| v as usize)
    }

    /// Reply to a received message on a channel.
    ///
    /// Must be called on the same channel end that previously received a
    /// message via [`receive`]. The reply is delivered to the sender of
    /// the original message (if it called [`call`]).
    ///
    /// # Errors
    ///
    /// - `InvalidArgument` — no pending message to reply to.
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

/// Process lifecycle operations.
///
/// Provides functions for creating, starting, waiting on, and inspecting
/// processes. A new process is created in the scheduler but not started
/// until [`start`] loads an ELF binary from the initrd.
pub mod process {
    use super::*;

    /// Create a new process in the scheduler.
    ///
    /// The process is created but not yet running. Call [`start`] to load
    /// an ELF binary and begin execution.
    ///
    /// # Arguments
    ///
    /// * `name` — human-readable process name (for debugging / `ps`).
    ///
    /// # Returns
    ///
    /// The new process's task ID, usable with [`start`], [`wait`], and
    /// [`signal::kill`].
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
    /// The `task_id` must come from [`create`]. The `elf_filename` is the
    /// name of the ELF binary in the initrd archive (e.g., `"hello.elf"`).
    ///
    /// # Returns
    ///
    /// The task ID of the started process (same as `task_id`).
    ///
    /// # Errors
    ///
    /// - `NotFound` — `elf_filename` is not in the initrd.
    /// - `InvalidArgument` — `task_id` is not a valid process.
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
    ///
    /// This function does not return. The status code is stored in the
    /// task's exit status and can be retrieved by the parent via [`wait`].
    pub fn exit(status: u64) -> ! {
        unsafe {
            raw::syscall1(number::PROCESS_EXIT, status);
        }
        unreachable!()
    }

    /// Wait for a child process to exit.
    ///
    /// Blocks the calling thread until the child exits or the timeout expires.
    ///
    /// # Arguments
    ///
    /// * `task_id` — the child's task ID (from [`create`]).
    /// * `timeout_ticks` — maximum timer ticks to wait. Pass `u64::MAX` to
    ///   block indefinitely.
    ///
    /// # Returns
    ///
    /// The child's exit status (the value passed to [`exit`]).
    ///
    /// # Errors
    ///
    /// - `Timeout` — the child did not exit within the deadline.
    /// - `InvalidArgument` — `task_id` is not a child of the caller.
    pub fn wait(task_id: u64, timeout_ticks: u64) -> Result<u64, Error> {
        let raw = unsafe { raw::syscall2(number::PROCESS_WAIT, task_id, timeout_ticks) };
        result(raw)
    }

    /// Get the current process ID (task ID).
    ///
    /// Always succeeds. Returns the task ID assigned to this process at
    /// creation time.
    pub fn getpid() -> u64 {
        let raw = unsafe { raw::syscall0(number::GETPID) };
        // Always succeeds — kernel returns the current task ID.
        raw as u64
    }

    /// Get the parent process ID.
    ///
    /// Returns the task ID of the process that created this one, or 0 if
    /// this is the root task (init).
    pub fn getppid() -> u64 {
        let raw = unsafe { raw::syscall0(number::GETPPID) };
        raw as u64
    }

    /// Get the real user ID of the current process.
    ///
    /// Always succeeds. Currently returns 0 (root).
    pub fn getuid() -> u64 {
        let raw = unsafe { raw::syscall0(number::GETUID) };
        raw as u64
    }

    /// Get the real group ID of the current process.
    ///
    /// Always succeeds. Currently returns 0 (root).
    pub fn getgid() -> u64 {
        let raw = unsafe { raw::syscall0(number::GETGID) };
        raw as u64
    }

    /// Set the real user ID of the current process.
    pub fn setuid(uid: u64) -> Result<(), Error> {
        let raw = unsafe { raw::syscall1(number::SETUID, uid) };
        result(raw)?;
        Ok(())
    }

    /// Set the real group ID of the current process.
    pub fn setgid(gid: u64) -> Result<(), Error> {
        let raw = unsafe { raw::syscall1(number::SETGID, gid) };
        result(raw)?;
        Ok(())
    }

    /// Get the current thread ID.
    ///
    /// In OpenOS, each thread has its own task ID. Always succeeds.
    pub fn gettid() -> u64 {
        let raw = unsafe { raw::syscall0(number::GETTID) };
        raw as u64
    }

    /// Information about a task, returned by [`list_tasks`].
    #[derive(Debug, Clone)]
    pub struct TaskInfo {
        /// Unique task identifier (same as the value returned by [`create`]).
        pub id: u64,
        /// Current scheduling state.
        pub state: TaskState,
        /// Scheduling priority (0 = lowest, 255 = highest).
        pub priority: u8,
        /// Human-readable task name (set at creation time).
        pub name: alloc::string::String,
    }

    /// Scheduling state of a task.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum TaskState {
        /// Task is in the run queue but not currently executing.
        Ready,
        /// Task is currently executing on a CPU.
        Running,
        /// Task is blocked waiting for an IPC message, event, or sleep.
        Blocked,
        /// Task has exited and its resources are being reclaimed.
        Terminated,
    }

    /// List all tasks in the system.
    ///
    /// Returns a [`Vec<TaskInfo>`] for every task regardless of state.
    /// Internally, the kernel writes packed 40-byte entries into a
    /// caller-supplied buffer. The SDK handles buffer sizing automatically,
    /// retrying with a larger buffer if the initial allocation is too small.
    ///
    /// # Errors
    ///
    /// - `OutOfMemory` — the kernel could not write the task list.
    pub fn list_tasks() -> Result<alloc::vec::Vec<TaskInfo>, Error> {
        // Size of each serialized task entry.
        const ENTRY_SIZE: usize = 40;
        // Initial buffer: enough for 64 tasks.
        let mut buf = [0u8; 64 * ENTRY_SIZE];
        let raw = unsafe {
            raw::syscall2(
                number::LIST_TASKS,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
            )
        };
        if raw < 0 {
            return Err(Error::from_raw(raw));
        }
        let count = raw as usize;
        let needed = count * ENTRY_SIZE;
        if needed > buf.len() {
            // Buffer too small — retry with a larger buffer.
            let mut big_buf = alloc::vec![0u8; needed];
            let raw2 = unsafe {
                raw::syscall2(
                    number::LIST_TASKS,
                    big_buf.as_mut_ptr() as u64,
                    big_buf.len() as u64,
                )
            };
            if raw2 < 0 {
                return Err(Error::from_raw(raw2));
            }
            let count2 = raw2 as usize;
            return parse_task_entries(&big_buf, count2);
        }
        parse_task_entries(&buf, count)
    }

    /// Parse packed task entries from a buffer.
    fn parse_task_entries(buf: &[u8], count: usize) -> Result<alloc::vec::Vec<TaskInfo>, Error> {
        const ENTRY_SIZE: usize = 40;
        let mut tasks = alloc::vec::Vec::with_capacity(count);
        for i in 0..count {
            let base = i * ENTRY_SIZE;
            if base + ENTRY_SIZE > buf.len() {
                break;
            }
            let id = u64::from_le_bytes(buf[base..base + 8].try_into().unwrap_or([0; 8]));
            let state = match buf[base + 8] {
                0 => TaskState::Ready,
                1 => TaskState::Running,
                2 => TaskState::Blocked,
                3 => TaskState::Terminated,
                _ => TaskState::Ready,
            };
            let priority = buf[base + 9];
            // Name starts at offset 12, max 32 bytes.
            let name_bytes = &buf[base + 12..base + 44];
            let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(32);
            let name = core::str::from_utf8(&name_bytes[..name_len])
                .unwrap_or("?")
                .into();
            tasks.push(TaskInfo {
                id,
                state,
                priority,
                name,
            });
        }
        Ok(tasks)
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

    /// Create a new thread in the current process.
    ///
    /// `entry` is the virtual address of the thread's entry point function.
    /// `stack` is the virtual address of the top of the thread's stack.
    ///
    /// Returns the new thread's task ID on success.
    pub fn create(entry: usize, stack: usize) -> Result<u64, Error> {
        let raw = unsafe { raw::syscall2(number::THREAD_CREATE, entry as u64, stack as u64) };
        result(raw)
    }

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

/// Memory management operations.
///
/// Provides low-level memory primitives: program break adjustment
/// and virtual memory mapping/unmapping.
pub mod memory {
    use super::*;

    /// Readable mapping.
    pub const MAP_READ: u32 = 1;
    /// Writable mapping.
    pub const MAP_WRITE: u32 = 2;
    /// Executable mapping.
    pub const MAP_EXEC: u32 = 4;

    /// Set the program break (heap end) to `addr`.
    ///
    /// Returns the new program break on success (which may differ from the
    /// requested address if the kernel rounds to a page boundary).
    /// Pass 0 to query the current break without changing it.
    pub fn brk(addr: usize) -> Result<usize, Error> {
        let raw = unsafe { raw::syscall1(number::BRK, addr as u64) };
        result(raw).map(|v| v as usize)
    }

    /// Map a region of virtual memory.
    ///
    /// `addr` is a hint for the desired virtual address (0 to let the kernel choose).
    /// `len` is the length in bytes (rounded up to page size by the kernel).
    /// `flags` is a combination of `MAP_READ`, `MAP_WRITE`, `MAP_EXEC`.
    ///
    /// Returns the virtual address of the mapped region on success.
    pub fn mmap(addr: usize, len: usize, flags: u32) -> Result<usize, Error> {
        let raw = unsafe { raw::syscall3(number::MMAP, addr as u64, len as u64, flags as u64) };
        result(raw).map(|v| v as usize)
    }

    /// Unmap a region of virtual memory.
    ///
    /// `addr` and `len` must match a previous `mmap` call.
    pub fn munmap(addr: usize, len: usize) -> Result<(), Error> {
        let raw = unsafe { raw::syscall2(number::MUNMAP, addr as u64, len as u64) };
        result(raw)?;
        Ok(())
    }

    /// Change protection on a mapped memory region.
    ///
    /// `addr` must be page-aligned. `len` is rounded up to page boundary.
    /// `prot` is a bitmask: bit 0=read, bit 1=write, bit 2=exec.
    pub fn mprotect(addr: usize, len: usize, prot: u32) -> Result<(), Error> {
        let raw = unsafe { raw::syscall3(number::MPROTECT, addr as u64, len as u64, prot as u64) };
        result(raw)?;
        Ok(())
    }

    /// Protection flag: readable.
    pub const PROT_READ: u32 = 1;
    /// Protection flag: writable.
    pub const PROT_WRITE: u32 = 2;
    /// Protection flag: executable.
    pub const PROT_EXEC: u32 = 4;
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

/// Signal operations.
///
/// Signals provide a simple inter-process notification mechanism.
/// A signal can be sent to a process by task ID, and signal handlers
/// can be installed to run user-space code when a signal arrives.
pub mod signal {
    use super::*;

    /// Interrupt from keyboard (Ctrl-C).
    pub const SIGINT: u8 = 2;
    /// Kill signal (cannot be caught or ignored).
    pub const SIGKILL: u8 = 9;
    /// Broken pipe: write to pipe with no readers.
    pub const SIGPIPE: u8 = 13;
    /// Termination signal.
    pub const SIGTERM: u8 = 15;
    /// Child stopped or terminated.
    pub const SIGCHLD: u8 = 17;

    /// Default action — kernel handles the signal.
    pub const SIG_DFL: u64 = 0;
    /// Ignore the signal — silently discarded.
    pub const SIG_IGN: u64 = 1;

    /// Send a signal to a process.
    ///
    /// `pid` is the target task ID. `sig` is the signal number (1..=31).
    /// Returns `Ok(())` on success.
    pub fn kill(pid: u64, sig: u8) -> Result<(), Error> {
        let raw = unsafe { raw::syscall2(number::KILL, pid, sig as u64) };
        result(raw)?;
        Ok(())
    }

    /// Set the handler for a signal, returning the previous handler.
    ///
    /// `sig` is the signal number (1..=31). `handler` is the new handler
    /// address (`SIG_DFL` for default, `SIG_IGN` for ignore, or a user-space
    /// function address).
    ///
    /// Returns the previous handler address on success.
    pub fn signal(sig: u8, handler: u64) -> Result<u64, Error> {
        let raw = unsafe { raw::syscall2(number::SIGNAL, sig as u64, handler) };
        result(raw)
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

    /// Open a file by name for reading. Returns a file descriptor.
    ///
    /// The file must already exist on the filesystem.
    pub fn open(name: &str) -> Result<u64, Error> {
        let raw =
            unsafe { raw::syscall3(number::FS_OPEN, name.as_ptr() as u64, name.len() as u64, 0) };
        result(raw)
    }

    /// Open a file for writing, creating it if it does not exist.
    ///
    /// If the file already exists, it is truncated to zero length.
    /// Returns a file descriptor.
    pub fn create(name: &str) -> Result<u64, Error> {
        let raw =
            unsafe { raw::syscall3(number::FS_OPEN, name.as_ptr() as u64, name.len() as u64, 1) };
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

    /// Duplicate a file descriptor.
    ///
    /// If `new_fd` is already open, it is silently closed first.
    /// Returns `new_fd` on success.
    pub fn dup2(old_fd: u64, new_fd: u64) -> Result<u64, Error> {
        let raw = unsafe { raw::syscall2(number::DUP2, old_fd, new_fd) };
        result(raw)
    }

    /// Delete a file by path.
    pub fn unlink(path: &str) -> Result<(), Error> {
        let raw =
            unsafe { raw::syscall2(number::FS_UNLINK, path.as_ptr() as u64, path.len() as u64) };
        result(raw)?;
        Ok(())
    }

    /// Rename a file from old_path to new_path.
    pub fn rename(old_path: &str, new_path: &str) -> Result<(), Error> {
        let raw = unsafe {
            raw::syscall4(
                number::FS_RENAME,
                old_path.as_ptr() as u64,
                old_path.len() as u64,
                new_path.as_ptr() as u64,
                new_path.len() as u64,
            )
        };
        result(raw)?;
        Ok(())
    }

    /// Create a directory.
    pub fn mkdir(path: &str) -> Result<(), Error> {
        let raw =
            unsafe { raw::syscall3(number::FS_MKDIR, path.as_ptr() as u64, path.len() as u64, 0) };
        result(raw)?;
        Ok(())
    }

    /// Remove a directory.
    pub fn rmdir(path: &str) -> Result<(), Error> {
        let raw =
            unsafe { raw::syscall2(number::FS_RMDIR, path.as_ptr() as u64, path.len() as u64) };
        result(raw)?;
        Ok(())
    }

    /// Get file size (simplified stat).
    pub fn file_size(path: &str) -> Result<u64, Error> {
        let mut stat_buf = [0u8; 20];
        let raw = unsafe {
            raw::syscall3(
                number::FS_STAT,
                path.as_ptr() as u64,
                path.len() as u64,
                stat_buf.as_mut_ptr() as u64,
            )
        };
        result(raw)?;
        Ok(u64::from_le_bytes(
            stat_buf[0..8].try_into().unwrap_or([0; 8]),
        ))
    }

    /// Create a pipe. Returns `(read_fd, write_fd)`.
    ///
    /// Data written to `write_fd` can be read from `read_fd`.
    /// The pipe has a 4 KiB internal buffer.
    pub fn pipe() -> Result<(u64, u64), Error> {
        let mut fds = [0u64; 2];
        let raw = unsafe { raw::syscall1(number::FS_PIPE, fds.as_mut_ptr() as u64) };
        result(raw)?;
        Ok((fds[0], fds[1]))
    }

    /// Shared (read) lock for `flock`.
    pub const LOCK_SH: u64 = 1;
    /// Exclusive (write) lock for `flock`.
    pub const LOCK_EX: u64 = 2;
    /// Non-blocking flag for `flock` (ORed with LOCK_SH or LOCK_EX).
    pub const LOCK_NB: u64 = 4;
    /// Unlock for `flock`.
    pub const LOCK_UN: u64 = 8;

    /// Apply or remove an advisory lock on an open file descriptor.
    ///
    /// `operation` is a bitmask of `LOCK_SH`, `LOCK_EX`, `LOCK_UN`, and
    /// `LOCK_NB`. Locks are per-inode and advisory — other `flock` calls
    /// respect them but read/write operations do not.
    ///
    /// # Errors
    ///
    /// - `InvalidArgument` — `fd` is stdin/stdout or `operation` is invalid.
    /// - `NotFound` — `fd` is not a valid open file descriptor.
    /// - `WouldBlock` — the lock cannot be acquired and `LOCK_NB` was set.
    /// - `NotSupported` — `fd` refers to a pipe or unsupported file type.
    pub fn flock(fd: u64, operation: u64) -> Result<(), Error> {
        let raw = unsafe { raw::syscall2(number::FS_FLOCK, fd, operation) };
        result(raw)?;
        Ok(())
    }

    /// Create a named pipe (FIFO).
    ///
    /// Creates a FIFO special file at `path`. The FIFO acts like a pipe
    /// but exists in the filesystem namespace. Once created, it can be
    /// opened and used like a regular pipe.
    ///
    /// # Errors
    ///
    /// - `NotFound` — the parent directory does not exist.
    /// - `AlreadyExists` — a file or FIFO at `path` already exists.
    /// - `OutOfMemory` — the kernel is out of memory.
    pub fn mkfifo(path: &str) -> Result<(), Error> {
        let raw =
            unsafe { raw::syscall2(number::FS_MKFIFO, path.as_ptr() as u64, path.len() as u64) };
        result(raw)?;
        Ok(())
    }

    /// Get file status (metadata) for a path.
    ///
    /// Returns size, inode number, and file mode. Follows symbolic links.
    pub fn stat(path: &str) -> Result<Stat, Error> {
        let mut stat_buf = [0u8; 20];
        let raw = unsafe {
            raw::syscall3(
                number::FS_STAT,
                path.as_ptr() as u64,
                path.len() as u64,
                stat_buf.as_mut_ptr() as u64,
            )
        };
        result(raw)?;
        let size = u64::from_le_bytes(stat_buf[0..8].try_into().unwrap_or([0; 8]));
        let ino = u64::from_le_bytes(stat_buf[8..16].try_into().unwrap_or([0; 8]));
        let mode = u32::from_le_bytes(stat_buf[16..20].try_into().unwrap_or([0; 4]));
        Ok(Stat { size, ino, mode })
    }

    /// Get file status by file descriptor.
    ///
    /// Returns size, inode number, and file mode for an already-opened file.
    pub fn fstat(fd: u64) -> Result<Stat, Error> {
        let mut stat_buf = [0u8; 20];
        let raw = unsafe { raw::syscall2(number::FSTAT, fd, stat_buf.as_mut_ptr() as u64) };
        result(raw)?;
        let size = u64::from_le_bytes(stat_buf[0..8].try_into().unwrap_or([0; 8]));
        let ino = u64::from_le_bytes(stat_buf[8..16].try_into().unwrap_or([0; 8]));
        let mode = u32::from_le_bytes(stat_buf[16..20].try_into().unwrap_or([0; 4]));
        Ok(Stat { size, ino, mode })
    }

    /// Get file status without following symbolic links.
    ///
    /// Returns metadata for the link itself, not its target.
    pub fn lstat(path: &str) -> Result<Stat, Error> {
        let mut stat_buf = [0u8; 20];
        let raw = unsafe {
            raw::syscall3(
                number::LSTAT,
                path.as_ptr() as u64,
                path.len() as u64,
                stat_buf.as_mut_ptr() as u64,
            )
        };
        result(raw)?;
        let size = u64::from_le_bytes(stat_buf[0..8].try_into().unwrap_or([0; 8]));
        let ino = u64::from_le_bytes(stat_buf[8..16].try_into().unwrap_or([0; 8]));
        let mode = u32::from_le_bytes(stat_buf[16..20].try_into().unwrap_or([0; 4]));
        Ok(Stat { size, ino, mode })
    }

    /// Check file accessibility.
    ///
    /// `mode` is a bitmask of `F_OK`, `R_OK`, `W_OK`, `X_OK` from the
    /// [`access`] module. Returns `Ok(())` if the requested access is
    /// permitted, or an error otherwise.
    pub fn access(path: &str, mode: u64) -> Result<(), Error> {
        let raw = unsafe {
            raw::syscall3(
                number::ACCESS,
                path.as_ptr() as u64,
                path.len() as u64,
                mode,
            )
        };
        result(raw)?;
        Ok(())
    }

    /// Create a symbolic link.
    ///
    /// `target` is the path the link will point to. `link_path` is the
    /// location of the symbolic link itself.
    pub fn symlink(target: &str, link_path: &str) -> Result<(), Error> {
        let raw = unsafe {
            raw::syscall4(
                number::SYMLINK,
                target.as_ptr() as u64,
                target.len() as u64,
                link_path.as_ptr() as u64,
                link_path.len() as u64,
            )
        };
        result(raw)?;
        Ok(())
    }

    /// Read the target of a symbolic link.
    ///
    /// Returns the path that the symbolic link points to.
    pub fn readlink(path: &str) -> Result<alloc::vec::Vec<u8>, Error> {
        let mut buf = [0u8; 4096];
        let raw = unsafe {
            raw::syscall4(
                number::READLINK,
                path.as_ptr() as u64,
                path.len() as u64,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
            )
        };
        if raw < 0 {
            return Err(Error::from_raw(raw));
        }
        let len = raw as usize;
        Ok(buf[..len].to_vec())
    }

    /// Change file permissions (mode).
    ///
    /// `mode` should be a Unix permission bitmask (e.g., `0o644`).
    pub fn chmod(path: &str, mode: u32) -> Result<(), Error> {
        let raw = unsafe {
            raw::syscall3(
                number::CHMOD,
                path.as_ptr() as u64,
                path.len() as u64,
                mode as u64,
            )
        };
        result(raw)?;
        Ok(())
    }

    /// Set the file mode creation mask and return the previous mask.
    ///
    /// The umask is applied to the `mode` argument of [`create`] and
    /// similar operations. Pass `0o777` to query the current mask
    /// without changing it (returns the old mask).
    pub fn umask(mask: u32) -> u32 {
        let raw = unsafe { raw::syscall1(number::UMASK, mask as u64) };
        raw as u32
    }

    /// Read directory entries (null-terminated names).
    ///
    /// Returns a list of entry names found in the directory at `path`.
    pub fn readdir(path: &str) -> Result<alloc::vec::Vec<alloc::string::String>, Error> {
        let mut buf = [0u8; 4096];
        let raw = unsafe {
            raw::syscall3(
                number::FS_READDIR,
                path.as_ptr() as u64,
                path.len() as u64,
                buf.as_mut_ptr() as u64,
            )
        };
        if raw < 0 {
            return Err(Error::from_raw(raw));
        }
        let len = raw as usize;
        let mut entries = alloc::vec::Vec::new();
        let mut pos = 0;
        while pos < len {
            // Find null terminator
            let end = buf[pos..]
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(buf.len() - pos);
            if end == 0 {
                break;
            }
            if let Ok(s) = core::str::from_utf8(&buf[pos..pos + end]) {
                entries.push(alloc::string::String::from(s));
            }
            pos += end + 1; // skip the null byte
        }
        Ok(entries)
    }

    /// Read directory entries in `linux_dirent64` format.
    ///
    /// `fd` must be an open file descriptor for a directory. Returns
    /// raw bytes suitable for parsing as `linux_dirent64` entries.
    pub fn getdents64(fd: u64, buf: &mut [u8]) -> Result<usize, Error> {
        let raw = unsafe {
            raw::syscall3(
                number::GETDENTS64,
                fd,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
            )
        };
        result(raw).map(|v| v as usize)
    }
}

/// Shared memory operations.
///
/// Provides System V-style shared memory segments: create/attach/detach.
/// Shared memory allows multiple processes to access the same physical
/// memory pages.
pub mod shm {
    use super::*;

    /// Create a new shared memory segment with a given key (or `IPC_PRIVATE`).
    ///
    /// `size` is the requested size in bytes (rounded up to page granularity).
    /// `flags` is a bitmask: `IPC_CREAT` and/or `IPC_EXCL` can be ORed with
    /// permission bits (e.g., `0o644`).
    ///
    /// Returns the shared memory segment ID on success.
    pub fn shmget(key: u64, size: u64, flags: u64) -> Result<u64, Error> {
        let raw = unsafe { raw::syscall3(number::SHMGET, key, size, flags) };
        result(raw)
    }

    /// Attach a shared memory segment to the current process's address space.
    ///
    /// `shmid` is the segment ID from `shmget`. `addr` is the desired virtual
    /// address (0 to let the kernel choose). `flags` can include `SHM_RDONLY`
    /// (0x1000) for read-only access.
    ///
    /// Returns the virtual address where the segment was attached.
    pub fn shmat(shmid: u64, addr: u64, flags: u64) -> Result<u64, Error> {
        let raw = unsafe { raw::syscall3(number::SHMAT, shmid, addr, flags) };
        result(raw)
    }

    /// Detach a shared memory segment from the current process's address space.
    ///
    /// `addr` must be the virtual address returned by a previous `shmat` call.
    pub fn shmdt(addr: u64) -> Result<(), Error> {
        let raw = unsafe { raw::syscall1(number::SHMDT, addr) };
        result(raw)?;
        Ok(())
    }

    /// Flag for `shmget`: create the segment if it does not exist.
    pub const IPC_CREAT: u64 = 0x200;
    /// Flag for `shmget`: fail if the segment already exists.
    pub const IPC_EXCL: u64 = 0x400;
    /// Private key: create a new segment regardless of existing keys.
    pub const IPC_PRIVATE: u64 = 0;
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

    /// Bind a socket to a local port.
    ///
    /// `sock_fd` is the socket descriptor from `create_tcp`.
    /// `port` is the local port number to listen on.
    pub fn bind(sock_fd: u64, port: u16) -> Result<(), Error> {
        let raw = unsafe { raw::syscall2(number::BIND, sock_fd, port as u64) };
        result(raw)?;
        Ok(())
    }

    /// Put a bound socket into listening state.
    ///
    /// `sock_fd` is a socket that has been bound with `bind`.
    pub fn listen(sock_fd: u64) -> Result<(), Error> {
        let raw = unsafe { raw::syscall1(number::LISTEN, sock_fd) };
        result(raw)?;
        Ok(())
    }

    /// Accept an incoming connection on a listening socket.
    ///
    /// Blocks until a connection is available.
    /// Returns the new socket descriptor for the accepted connection.
    pub fn accept(sock_fd: u64) -> Result<u64, Error> {
        let raw = unsafe { raw::syscall1(number::ACCEPT, sock_fd) };
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

    /// Socket option level for generic socket options.
    pub const SOL_SOCKET: u32 = 1;
    /// Socket option level for TCP options.
    pub const IPPROTO_TCP: u32 = 6;

    /// Allow reuse of local addresses.
    pub const SO_REUSEADDR: u32 = 2;
    /// Disable Nagle's algorithm.
    pub const TCP_NODELAY: u32 = 1;
    /// Receive buffer size.
    pub const SO_RCVBUF: u32 = 8;
    /// Send buffer size.
    pub const SO_SNDBUF: u32 = 7;

    /// Set a socket option.
    ///
    /// `fd` is the socket descriptor. `level` is the option level
    /// (`SOL_SOCKET` or `IPPROTO_TCP`). `opt` is the option name.
    /// `val` is the option value as raw bytes (4 bytes for all current options).
    pub fn setsockopt(fd: u64, level: u32, opt: u32, val: &[u8]) -> Result<(), Error> {
        let raw = unsafe {
            raw::syscall5(
                number::SETSOCKOPT,
                fd,
                level as u64,
                opt as u64,
                val.as_ptr() as u64,
                val.len() as u64,
            )
        };
        result(raw)?;
        Ok(())
    }

    /// Get a socket option.
    ///
    /// `fd` is the socket descriptor. `level` is the option level
    /// (`SOL_SOCKET` or `IPPROTO_TCP`). `opt` is the option name.
    /// `val` is a buffer to receive the option value.
    ///
    /// Returns the number of bytes written to `val` on success.
    pub fn getsockopt(fd: u64, level: u32, opt: u32, val: &mut [u8]) -> Result<usize, Error> {
        let mut optlen = val.len() as u32;
        let raw = unsafe {
            raw::syscall5(
                number::GETSOCKOPT,
                fd,
                level as u64,
                opt as u64,
                val.as_mut_ptr() as u64,
                (&mut optlen as *mut u32) as u64,
            )
        };
        result(raw).map(|v| v as usize)
    }

    /// Get the remote address of a connected socket.
    ///
    /// `fd` is the socket descriptor. Writes the remote `(addr, port)` into
    /// the provided buffers (4 bytes address in network byte order, 2 bytes port).
    ///
    /// Returns `Ok(())` on success.
    pub fn getpeername(fd: u64) -> Result<(u32, u16), Error> {
        // The kernel expects a sockaddr structure: 2 bytes family, 2 bytes port, 4 bytes addr.
        let mut sockaddr = [0u8; 16];
        let mut addr_len: u64 = 16;
        let raw = unsafe {
            raw::syscall3(
                number::GETPEERNAME,
                fd,
                sockaddr.as_mut_ptr() as u64,
                (&mut addr_len as *mut u64) as u64,
            )
        };
        result(raw)?;
        let port = u16::from_be_bytes([sockaddr[2], sockaddr[3]]);
        let addr = u32::from_be_bytes([sockaddr[4], sockaddr[5], sockaddr[6], sockaddr[7]]);
        Ok((addr, port))
    }

    /// Get the local address of a bound socket.
    ///
    /// `fd` is the socket descriptor. Writes the local `(addr, port)` into
    /// the provided buffers (4 bytes address in network byte order, 2 bytes port).
    ///
    /// Returns `Ok(())` on success.
    pub fn getsockname(fd: u64) -> Result<(u32, u16), Error> {
        let mut sockaddr = [0u8; 16];
        let mut addr_len: u64 = 16;
        let raw = unsafe {
            raw::syscall3(
                number::GETSOCKNAME,
                fd,
                sockaddr.as_mut_ptr() as u64,
                (&mut addr_len as *mut u64) as u64,
            )
        };
        result(raw)?;
        let port = u16::from_be_bytes([sockaddr[2], sockaddr[3]]);
        let addr = u32::from_be_bytes([sockaddr[4], sockaddr[5], sockaddr[6], sockaddr[7]]);
        Ok((addr, port))
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

/// Scatter-gather I/O operations.
///
/// Provides `readv` and `writev` for reading/writing data in vector format
/// (a set of non-contiguous buffers) with a single system call.
pub mod io {
    use super::*;

    /// Read data into a scatter-gather vector of buffers.
    ///
    /// `fd` is the file descriptor. `iovs` is a slice of [`IoVec`] buffers
    /// that will be filled with data sequentially.
    ///
    /// Returns the total number of bytes read across all buffers.
    pub fn readv(fd: u64, iovs: &[IoVec]) -> Result<usize, Error> {
        if iovs.is_empty() {
            return Ok(0);
        }
        // Build the iovec array expected by the kernel:
        // each entry is [base (u64), len (u64)] = 16 bytes.
        let mut raw_iov = [0u8; 1024 * 16];
        let count = iovs.len().min(1024);
        for (i, iov) in iovs[..count].iter().enumerate() {
            let base = i * 16;
            raw_iov[base..base + 8].copy_from_slice(&(iov.base as u64).to_le_bytes());
            raw_iov[base + 8..base + 16].copy_from_slice(&(iov.len as u64).to_le_bytes());
        }
        let raw =
            unsafe { raw::syscall3(number::READV, fd, raw_iov.as_ptr() as u64, count as u64) };
        result(raw).map(|v| v as usize)
    }

    /// Write data from a scatter-gather vector of buffers.
    ///
    /// `fd` is the file descriptor. `iovs` is a slice of [`IoVec`] buffers
    /// whose contents will be written sequentially.
    ///
    /// Returns the total number of bytes written across all buffers.
    pub fn writev(fd: u64, iovs: &[IoVec]) -> Result<usize, Error> {
        if iovs.is_empty() {
            return Ok(0);
        }
        // Build the iovec array expected by the kernel.
        let mut raw_iov = [0u8; 1024 * 16];
        let count = iovs.len().min(1024);
        for (i, iov) in iovs[..count].iter().enumerate() {
            let base = i * 16;
            raw_iov[base..base + 8].copy_from_slice(&(iov.base as u64).to_le_bytes());
            raw_iov[base + 8..base + 16].copy_from_slice(&(iov.len as u64).to_le_bytes());
        }
        let raw =
            unsafe { raw::syscall3(number::WRITEV, fd, raw_iov.as_ptr() as u64, count as u64) };
        result(raw).map(|v| v as usize)
    }
}

/// Resource usage and limit operations.
pub mod resource {
    use super::*;

    /// Resource usage for the calling process (self).
    pub const RUSAGE_SELF: u64 = 0;
    /// Resource usage for terminated child processes.
    pub const RUSAGE_CHILDREN: u64 = 1;

    /// Get resource usage statistics.
    ///
    /// `who` is either `RUSAGE_SELF` or `RUSAGE_CHILDREN`. Returns a buffer
    /// of 144 bytes containing resource usage counters (currently all zeros).
    pub fn getrusage(who: u64) -> Result<[u8; 144], Error> {
        let mut buf = [0u8; 144];
        let raw = unsafe { raw::syscall2(number::GETRUSAGE, who, buf.as_mut_ptr() as u64) };
        result(raw)?;
        Ok(buf)
    }

    /// Get/set resource limits for a process.
    ///
    /// `pid` is the target process (0 for self). `resource` is the resource
    /// identifier. `new_limit` is an optional pointer to a 16-byte rlimit
    /// struct `[cur(u64), max(u64)]` to set new limits (pass 0 to query).
    /// `old_limit` is a buffer to receive the previous limits (16 bytes).
    pub fn prlimit(
        pid: u64,
        resource: u64,
        new_limit: Option<&[u8; 16]>,
        old_limit: &mut [u8; 16],
    ) -> Result<(), Error> {
        let new_ptr = match new_limit {
            Some(nl) => nl.as_ptr() as u64,
            None => 0,
        };
        let raw = unsafe {
            raw::syscall4(
                number::PRLIMIT,
                pid,
                resource,
                new_ptr,
                old_limit.as_mut_ptr() as u64,
            )
        };
        result(raw)?;
        Ok(())
    }
}

/// Miscellaneous system operations.
///
/// Provides wrappers for miscellaneous syscalls (ioctl, getrandom,
/// membarrier, madvise, sched_yield, timer_create, etc.).
pub mod misc {
    use super::*;

    /// I/O control operation on a file descriptor.
    ///
    /// `fd` is the target file descriptor. `request` is the device-specific
    /// ioctl request code. `argp` is an optional pointer to request data.
    pub fn ioctl(fd: u64, request: u64, argp: u64) -> Result<u64, Error> {
        let raw = unsafe { raw::syscall3(number::IOCTL, fd, request, argp) };
        result(raw)
    }

    /// Fill a buffer with pseudo-random bytes.
    ///
    /// Returns the number of bytes written (always equals `buf.len()` on
    /// success).
    pub fn getrandom(buf: &mut [u8]) -> Result<usize, Error> {
        let raw =
            unsafe { raw::syscall2(number::GETRANDOM, buf.as_mut_ptr() as u64, buf.len() as u64) };
        result(raw).map(|v| v as usize)
    }

    /// Issue a memory barrier command.
    ///
    /// `cmd` is the membarrier operation (0 = query support, 1 = global,
    /// 2 = private expedited, etc.).
    pub fn membarrier(cmd: u64) -> Result<u64, Error> {
        let raw = unsafe { raw::syscall1(number::MEMBARRIER, cmd) };
        result(raw)
    }

    /// Advise the kernel about memory usage patterns.
    ///
    /// `addr` and `len` describe the memory region. `advice` is a hint
    /// (e.g., MADV_NORMAL = 0, MADV_RANDOM = 1, MADV_SEQUENTIAL = 2).
    pub fn madvise(addr: usize, len: usize, advice: u32) -> Result<(), Error> {
        let raw = unsafe { raw::syscall3(number::MADVISE, addr as u64, len as u64, advice as u64) };
        result(raw)?;
        Ok(())
    }

    /// Yield the CPU to other tasks.
    ///
    /// The current thread is moved to the back of the run queue.
    pub fn sched_yield() {
        unsafe {
            raw::syscall0(number::SCHED_YIELD);
        }
    }

    /// Create an interval timer.
    ///
    /// `clock_id` specifies the clock (typically `CLOCK_MONOTONIC` = 0).
    /// `sev_ptr` is an optional pointer to a `sigevent` structure (pass 0
    /// for default behavior).
    ///
    /// Returns the timer ID on success.
    pub fn timer_create(clock_id: u32, sev_ptr: u64) -> Result<u64, Error> {
        let raw = unsafe { raw::syscall2(number::TIMER_CREATE, clock_id as u64, sev_ptr) };
        result(raw)
    }

    /// Arm or disarm an interval timer.
    ///
    /// `timer_id` is from `timer_create`. `flags` is 0 for relative,
    /// TIMER_ABSTIME (1) for absolute. `new_ptr` points to an `itimerspec`
    /// (two 16-byte `timespec` structs = 32 bytes) in user space.
    ///
    /// Returns 0 on success.
    pub fn timer_settime(timer_id: u64, flags: u64, new_ptr: u64) -> Result<(), Error> {
        let raw = unsafe { raw::syscall3(number::TIMER_SETTIME, timer_id, flags, new_ptr) };
        result(raw)?;
        Ok(())
    }

    /// Query the current state of an interval timer.
    ///
    /// `timer_id` is from `timer_create`. `curr_ptr` points to a user-space
    /// buffer (32 bytes) to receive the `itimerspec` data.
    ///
    /// Returns 0 on success.
    pub fn timer_gettime(timer_id: u64, curr_ptr: u64) -> Result<(), Error> {
        let raw = unsafe { raw::syscall2(number::TIMER_GETTIME, timer_id, curr_ptr) };
        result(raw)?;
        Ok(())
    }

    /// Drain one entry from the kernel syslog buffer.
    ///
    /// Writes one syslog entry into `buf`. Returns the number of bytes
    /// written, or 0 if no entries are available.
    pub fn syslog_drain(buf: &mut [u8]) -> Result<usize, Error> {
        let raw = unsafe {
            raw::syscall2(
                number::SYSLOG_DRAIN,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
            )
        };
        result(raw).map(|v| v as usize)
    }

    /// Duplicate a file descriptor with flags.
    ///
    /// Similar to `dup2` but also sets flags on the new descriptor.
    /// `old_fd` is the source, `new_fd` is the target (closed first if
    /// open), and `flags` is a bitmask of O_* flags (e.g., O_CLOEXEC = 0x40000).
    ///
    /// Returns the new file descriptor (`new_fd`) on success.
    pub fn dup3(old_fd: u64, new_fd: u64, flags: u64) -> Result<u64, Error> {
        let raw = unsafe { raw::syscall3(number::DUP3, old_fd, new_fd, flags) };
        result(raw)
    }

    /// Create an epoll instance.
    ///
    /// `size` is a hint for the number of file descriptors (ignored by
    /// modern kernels). Returns an epoll file descriptor.
    pub fn epoll_create(size: u64) -> Result<u64, Error> {
        let raw = unsafe { raw::syscall1(number::EPOLL_CREATE, size) };
        result(raw)
    }

    /// Control an epoll instance: register, modify, or remove a file
    /// descriptor.
    ///
    /// `epfd` is the epoll fd from `epoll_create`. `op` is EPOLL_CTL_ADD (1),
    /// EPOLL_CTL_MOD (2), or EPOLL_CTL_DEL (3). `fd` is the target file
    /// descriptor. `event` is a pointer to an `epoll_event` struct (12 bytes).
    pub fn epoll_ctl(epfd: u64, op: u64, fd: u64, event: u64) -> Result<(), Error> {
        let raw = unsafe { raw::syscall4(number::EPOLL_CTL, epfd, op, fd, event) };
        result(raw)?;
        Ok(())
    }

    /// Wait for events on an epoll instance.
    ///
    /// `epfd` is the epoll fd. `events` is a buffer to receive `epoll_event`
    /// structs (12 bytes each). `maxevents` is the number of events that fit.
    /// `timeout` is the timeout in milliseconds (-1 = infinite, 0 = non-blocking).
    ///
    /// Returns the number of ready file descriptors.
    pub fn epoll_wait(
        epfd: u64,
        events: &mut [u8],
        maxevents: u64,
        timeout: i32,
    ) -> Result<usize, Error> {
        let raw = unsafe {
            raw::syscall4(
                number::EPOLL_WAIT,
                epfd,
                events.as_mut_ptr() as u64,
                maxevents,
                timeout as u64,
            )
        };
        result(raw).map(|v| v as usize)
    }
}

/// Device I/O operations (port I/O and MMIO).
///
/// Provides raw hardware access for user-space device drivers.
pub mod device {
    use super::*;

    /// Read a value from an I/O port.
    ///
    /// `size` must be 1 (u8), 2 (u16), or 4 (u32).
    /// Returns the value read as a `u64`.
    pub fn port_in(port: u16, size: u64) -> Result<u64, Error> {
        let raw = unsafe { raw::syscall2(number::PORT_IN, port as u64, size) };
        result(raw)
    }

    /// Write a value to an I/O port.
    ///
    /// `size` must be 1 (u8), 2 (u16), or 4 (u32).
    pub fn port_out(port: u16, value: u64, size: u64) -> Result<(), Error> {
        let raw = unsafe { raw::syscall3(number::PORT_OUT, port as u64, value, size) };
        result(raw)?;
        Ok(())
    }

    /// Map a physical MMIO region into the current task's address space.
    ///
    /// Both `phys_addr` and `size` must be page-aligned (4 KiB).
    /// Returns the virtual address of the mapped region.
    pub fn mmio_map(phys_addr: u64, size: u64) -> Result<u64, Error> {
        let raw = unsafe { raw::syscall2(number::MMIO_MAP, phys_addr, size) };
        result(raw)
    }

    /// Unmap a previously mapped MMIO region.
    ///
    /// `virt_addr` and `size` must match a previous `mmio_map` call.
    pub fn mmio_unmap(virt_addr: u64, size: u64) -> Result<(), Error> {
        let raw = unsafe { raw::syscall2(number::MMIO_UNMAP, virt_addr, size) };
        result(raw)?;
        Ok(())
    }

    /// Wait for a hardware IRQ event.
    ///
    /// `handle` must reference an `IrqEvent` kernel object.
    /// `blocking`: if true, blocks until the IRQ fires; if false, returns
    /// `WouldBlock` immediately if not yet signaled.
    ///
    /// On success, returns the device data byte captured by the IRQ handler
    /// (e.g., a keyboard scancode for IRQ 1).
    pub fn irq_wait(handle: Handle, blocking: bool) -> Result<u64, Error> {
        let flags: u64 = if blocking { 1 } else { 0 };
        let raw = unsafe { raw::syscall2(number::IRQ_WAIT, handle.as_raw(), flags) };
        result(raw)
    }
}

/// Service discovery operations.
///
/// Allows tasks to register and discover named service endpoints.
pub mod service {
    use super::*;

    /// Register a named service endpoint in the current task's namespace.
    ///
    /// `name` is the service name (e.g., "devmgr", "keyboard").
    /// `handle` is the Channel server-end handle that clients will connect to.
    pub fn register(name: &str, handle: Handle) -> Result<(), Error> {
        let raw = unsafe {
            raw::syscall3(
                number::ENDPOINT_REGISTER,
                name.as_ptr() as u64,
                name.len() as u64,
                handle.as_raw(),
            )
        };
        result(raw)?;
        Ok(())
    }

    /// Discover a named service endpoint in the current task's namespace.
    ///
    /// Returns the Channel server-end handle registered under `name`.
    pub fn discover(name: &str) -> Result<Handle, Error> {
        let raw = unsafe {
            raw::syscall2(
                number::ENDPOINT_DISCOVER,
                name.as_ptr() as u64,
                name.len() as u64,
            )
        };
        result(raw).map(Handle::from_raw)
    }
}

/// Environment variable operations.
///
/// Each task has its own key-value environment. Variables are inherited
/// when a new process is created (not yet implemented).
pub mod env {
    use super::*;

    /// Get an environment variable by key.
    ///
    /// Returns `Ok(Some(value))` if the key exists, `Ok(None)` if it does not.
    pub fn get(key: &str) -> Result<Option<alloc::string::String>, Error> {
        // Use a 1 KiB buffer for the value — sufficient for most env vars.
        let mut val_buf = [0u8; 1024];
        let raw = unsafe {
            raw::syscall4(
                number::ENV_GET,
                key.as_ptr() as u64,
                key.len() as u64,
                val_buf.as_mut_ptr() as u64,
                val_buf.len() as u64,
            )
        };
        if raw < 0 {
            return Err(Error::from_raw(raw));
        }
        if raw == 0 {
            return Ok(None);
        }
        let len = raw as usize;
        let value = core::str::from_utf8(&val_buf[..len]).map_err(|_| Error::InvalidArgument)?;
        Ok(Some(alloc::string::String::from(value)))
    }

    /// Set an environment variable.
    ///
    /// If the key already exists, its value is overwritten.
    pub fn set(key: &str, value: &str) -> Result<(), Error> {
        let raw = unsafe {
            raw::syscall4(
                number::ENV_SET,
                key.as_ptr() as u64,
                key.len() as u64,
                value.as_ptr() as u64,
                value.len() as u64,
            )
        };
        result(raw)?;
        Ok(())
    }

    /// Change the current working directory.
    ///
    /// The path must exist and refer to a directory.
    pub fn chdir(path: &str) -> Result<(), Error> {
        let raw = unsafe { raw::syscall2(number::CHDIR, path.as_ptr() as u64, path.len() as u64) };
        result(raw)?;
        Ok(())
    }

    /// Get the current working directory.
    ///
    /// Returns the absolute path of the current working directory.
    pub fn cwd() -> Result<alloc::string::String, Error> {
        let mut buf = [0u8; 4096];
        let raw =
            unsafe { raw::syscall2(number::GETCWD, buf.as_mut_ptr() as u64, buf.len() as u64) };
        if raw < 0 {
            return Err(Error::from_raw(raw));
        }
        let len = raw as usize;
        let path = core::str::from_utf8(&buf[..len]).map_err(|_| Error::InvalidArgument)?;
        Ok(alloc::string::String::from(path))
    }
}

/// Time operations via kernel syscalls.
///
/// Provides access to the kernel's monotonic clock and a convenience sleep
/// function that yields the CPU to other tasks.
pub mod time {
    use super::*;

    /// Sleep for the given number of timer ticks.
    ///
    /// The calling thread is suspended for at least `ticks` timer intervals
    /// and then rescheduled. A tick of 0 yields without blocking.
    pub fn sleep(ticks: u64) {
        unsafe {
            raw::syscall1(number::SLEEP, ticks);
        }
    }

    /// Clock identifier for the monotonic clock (ticks since boot).
    pub const CLOCK_MONOTONIC: u32 = 0;

    /// A time value with nanosecond precision.
    ///
    /// Represents elapsed time since boot for the monotonic clock.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Timespec {
        /// Whole seconds.
        pub sec: u64,
        /// Nanoseconds remaining (0..999_999_999).
        pub nsec: u64,
    }

    /// Get the current time for a clock.
    ///
    /// Only `CLOCK_MONOTONIC` (clock_id == 0) is supported. The time is
    /// derived from the kernel's timer tick counter.
    ///
    /// Returns `Some(Timespec)` on success, `None` if the clock_id is
    /// invalid or the user-space pointer is bad.
    pub fn clock_gettime(clock_id: u32) -> Option<Timespec> {
        let mut buf = [0u8; 16];
        let raw = unsafe {
            raw::syscall2(
                number::CLOCK_GETTIME,
                clock_id as u64,
                buf.as_mut_ptr() as u64,
            )
        };
        if raw < 0 {
            return None;
        }
        let sec = u64::from_le_bytes(buf[0..8].try_into().unwrap_or([0; 8]));
        let nsec = u64::from_le_bytes(buf[8..16].try_into().unwrap_or([0; 8]));
        Some(Timespec { sec, nsec })
    }

    /// Sleep for the given number of milliseconds.
    ///
    /// Converts milliseconds to timer ticks (assuming 100 Hz timer = 10 ms/tick)
    /// and calls the kernel's `SYS_SLEEP` syscall, which yields the CPU to other
    /// tasks until the requested duration has elapsed.
    pub fn sleep_ms(ms: u64) {
        /// Milliseconds per timer tick (100 Hz -> 10 ms).
        const MS_PER_TICK: u64 = 10;
        let ticks = (ms + MS_PER_TICK - 1) / MS_PER_TICK;
        if ticks > 0 {
            unsafe {
                raw::syscall1(number::SLEEP, ticks);
            }
        }
    }

    /// Return the raw monotonic tick count.
    ///
    /// Uses `clock_gettime` to read the monotonic clock, then converts
    /// the result back to ticks (sec * 100 + nsec / 10_000_000).
    pub fn ticks() -> u64 {
        /// Timer frequency in Hz.
        const TIMER_HZ: u64 = 100;
        /// Nanoseconds per tick (100 Hz -> 10_000_000 ns).
        const NS_PER_TICK: u64 = 1_000_000_000 / TIMER_HZ;

        let Some(ts) = clock_gettime(CLOCK_MONOTONIC) else {
            return 0;
        };
        ts.sec * TIMER_HZ + ts.nsec / NS_PER_TICK
    }
}
