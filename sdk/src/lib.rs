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

extern crate alloc;

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

    /// Get the current process ID (task ID).
    pub fn getpid() -> u64 {
        let raw = unsafe { raw::syscall0(number::GETPID) };
        // Always succeeds — kernel returns the current task ID.
        raw as u64
    }

    /// Get the parent process ID.
    ///
    /// Returns the parent task's ID, or 0 if the process has no parent.
    pub fn getppid() -> u64 {
        let raw = unsafe { raw::syscall0(number::GETPPID) };
        raw as u64
    }

    /// Information about a task, returned by `list_tasks`.
    #[derive(Debug, Clone)]
    pub struct TaskInfo {
        /// Unique task identifier.
        pub id: u64,
        /// Current scheduling state.
        pub state: TaskState,
        /// Scheduling priority (0 = lowest, 255 = highest).
        pub priority: u8,
        /// Task name.
        pub name: alloc::string::String,
    }

    /// Scheduling state of a task.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum TaskState {
        /// Task is ready to run.
        Ready,
        /// Task is currently running.
        Running,
        /// Task is blocked waiting for an event.
        Blocked,
        /// Task has terminated.
        Terminated,
    }

    /// List all tasks in the system.
    ///
    /// Returns a vector of `TaskInfo` for every task (ready, running, blocked).
    /// The syscall writes packed 40-byte entries into a buffer:
    /// `[u64 id][u8 state][u8 priority][u16 reserved][u8 name[32]]`.
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
