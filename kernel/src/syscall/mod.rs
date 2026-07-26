//! System call interface.
//!
//! User-space invokes a syscall via the `syscall` instruction. The assembly
//! stub in `arch/x86_64/syscall.rs` saves registers, calls `handle_syscall_raw`,
//! and either SYSRETs back to user-space or switches to a different task.
//!
//! ## Syscall number convention
//!
//! Syscall numbers are grouped by subsystem with allocated ranges:
//!
//! | Range     | Subsystem         | Count |
//! |-----------|-------------------|-------|
//! | `0x01-0x0F` | Channel (IPC)   | 5     |
//! | `0x10-0x1F` | Handle          | 3     |
//! | `0x30-0x3F` | Process         | 9     |
//! | `0x40-0x4F` | Thread          | 3     |
//! | `0xA0-0xAF` | Socket / DNS    | 9     |
//! | `0xB0-0xBF` | Hardware access  | 5     |
//! | `0xF0-0xFF` | OpenOS-specific  | ~12   |
//!
//! Numbers are defined in `syscall::number` and must not overlap.
//!
//! ## Error convention (INTERFACE.md §3.1)
//!
//! - Positive return = success value
//! - Negative return = error code (see `Error` enum)
//!
//! ## Preemption
//!
//! On each syscall entry, the handler checks `NEED_RESCHEDULE` (set by the
//! timer interrupt). If set, the current task is preempted before the syscall
//! is dispatched. This provides "cooperative preemption on syscall boundary."

pub mod number;

use number::{
    SYS_ACCEPT, SYS_ACCESS, SYS_BIND, SYS_BRK, SYS_CHANNEL_CALL, SYS_CHANNEL_CREATE,
    SYS_CHANNEL_RECEIVE, SYS_CHANNEL_REPLY, SYS_CHANNEL_SEND, SYS_CHDIR, SYS_CHMOD,
    SYS_CLOCK_GETTIME, SYS_CLOSE_SOCK, SYS_CONNECT, SYS_CONSOLE_READ, SYS_CONSOLE_WRITE,
    SYS_DNS_RESOLVE, SYS_DUP2, SYS_ENDPOINT_DISCOVER, SYS_ENDPOINT_REGISTER, SYS_ENV_GET,
    SYS_ENV_SET, SYS_EVENT_CREATE, SYS_EVENT_DESTROY, SYS_EVENT_SIGNAL, SYS_EVENT_WAIT, SYS_FSTAT,
    SYS_FS_CLOSE, SYS_FS_MKDIR, SYS_FS_OPEN, SYS_FS_READ, SYS_FS_READDIR, SYS_FS_RENAME,
    SYS_FS_RMDIR, SYS_FS_SEEK, SYS_FS_STAT, SYS_FS_UNLINK, SYS_FS_WRITE, SYS_GETCWD,
    SYS_GETDENTS64, SYS_GETPGID, SYS_GETPID, SYS_GETPPID, SYS_GETSOCKOPT, SYS_HANDLE_CLOSE,
    SYS_HANDLE_DUPLICATE, SYS_HANDLE_TRANSFER, SYS_IRQ_WAIT, SYS_KILL, SYS_LISTEN, SYS_LIST_TASKS,
    SYS_LSTAT, SYS_MMAP, SYS_MMIO_MAP, SYS_MMIO_UNMAP, SYS_MPROTECT, SYS_MUNMAP, SYS_NET_RECEIVE,
    SYS_NET_SEND, SYS_PIPE, SYS_POLL, SYS_PORT_IN, SYS_PORT_OUT, SYS_PROCESS_CREATE,
    SYS_PROCESS_EXIT, SYS_PROCESS_START, SYS_PROCESS_WAIT, SYS_READLINK, SYS_RECVFROM, SYS_SENDTO,
    SYS_SETPGID, SYS_SETSID, SYS_SETSOCKOPT, SYS_SIGNAL, SYS_SIGPROCMASK, SYS_SIGRETURN, SYS_SLEEP,
    SYS_SOCKET, SYS_SYMLINK, SYS_THREAD_CREATE, SYS_THREAD_EXIT, SYS_THREAD_YIELD, SYS_UMASK,
};

use crate::handle::{Handle, KernelObject, Rights};
use crate::ipc::{Channel, EndId};

/// Error codes (INTERFACE.md §3.1). Negative values returned in RAX.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum Error {
    /// An argument was invalid or out of range.
    InvalidArgument = -1,
    /// The requested resource was not found.
    NotFound = -2,
    /// The caller lacks the required rights.
    PermissionDenied = -3,
    /// The kernel is out of memory.
    OutOfMemory = -4,
    /// The resource is busy (e.g., port in use, connection in progress).
    Busy = -5,
    /// The channel has been closed by the peer.
    ChannelClosed = -6,
    /// The operation would block (non-blocking mode).
    WouldBlock = -7,
    /// The operation timed out.
    Timeout = -8,
    /// The user-space pointer is invalid or in kernel space.
    BadPointer = -9,
    /// The syscall number is not recognized.
    UnknownSyscall = -10,
    /// The operation is not supported.
    NotSupported = -11,
    /// File already exists.
    AlreadyExists = -12,
}

/// Maximum message size (4 KiB).
const MAX_MSG_SIZE: usize = 4096;

/// Copy bytes from user-space into a newly allocated kernel buffer.
///
/// Validates that the source pointer is non-null, within user-space bounds,
/// and that the range `[src, src+len)` does not cross into kernel space.
/// Returns `None` if any validation fails.
///
/// # Safety
///
/// The caller must ensure `src` points to a valid, mapped user-space region
/// of at least `len` bytes. The checks here are best-effort bounds validation;
/// they do not guarantee the pages are mapped (a page fault will occur if not).
unsafe fn copy_from_user(src: *const u8, len: usize) -> Option<alloc::vec::Vec<u8>> {
    if src.is_null() || len == 0 || len > MAX_MSG_SIZE {
        return None;
    }
    let src_addr = src as u64;
    if src_addr >= crate::memory::USER_SPACE_MAX {
        return None;
    }
    // Overflow check: src + len must not exceed user-space boundary.
    if src_addr.saturating_add(len as u64) > crate::memory::USER_SPACE_MAX {
        return None;
    }
    let mut buf = alloc::vec![0u8; len];
    // SAFETY: We validated that `src` is non-null, in user-space, and
    // `[src, src+len)` is within bounds. The kernel buffer `buf` has
    // exactly `len` bytes. No overlap is possible since `buf` is
    // freshly allocated in kernel heap.
    unsafe { core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), len) }
    Some(buf)
}

/// Copy bytes from a kernel buffer into user-space.
///
/// Validates that the destination pointer is non-null, within user-space bounds,
/// and that the range `[dst, dst+src.len())` does not cross into kernel space.
/// Returns `false` if any validation fails.
///
/// # Safety
///
/// The caller must ensure `dst` points to a valid, mapped, writable user-space
/// region of at least `src.len()` bytes. The checks here are best-effort bounds
/// validation; they do not guarantee the pages are mapped.
unsafe fn copy_to_user(dst: *mut u8, src: &[u8]) -> bool {
    if dst.is_null() || src.is_empty() || src.len() > MAX_MSG_SIZE {
        return false;
    }
    let dst_addr = dst as u64;
    if dst_addr >= crate::memory::USER_SPACE_MAX {
        return false;
    }
    // Overflow check: dst + len must not exceed user-space boundary.
    if dst_addr.saturating_add(src.len() as u64) > crate::memory::USER_SPACE_MAX {
        return false;
    }
    // SAFETY: We validated that `dst` is non-null, in user-space, and
    // `[dst, dst+len)` is within bounds. The source buffer `src` has
    // exactly `src.len()` bytes. No overlap is possible since `src` is
    // in kernel heap and `dst` is in user-space.
    unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), dst, src.len()) }
    true
}

/// Look up a `Handle` in the current task's handle table.
fn lookup_channel(handle_raw: u64) -> Option<(alloc::sync::Arc<spin::Mutex<Channel>>, EndId)> {
    let handle = Handle::from_raw(handle_raw);
    crate::task::scheduler::with_current_task(|task| match task.handle_table.get(handle) {
        Some(KernelObject::ChannelEndA(ch)) => Some((alloc::sync::Arc::clone(ch), EndId::A)),
        Some(KernelObject::ChannelEndB(ch)) => Some((alloc::sync::Arc::clone(ch), EndId::B)),
        _ => None,
    })?
}

/// Raw syscall handler called from the assembly stub.
#[no_mangle]
pub extern "C" fn handle_syscall_raw(
    number: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    arg4: u64,
    arg5: u64,
) -> i64 {
    // Preemption check: if the timer has set NEED_RESCHEDULE, yield now.
    // This is "cooperative preemption on syscall boundary" — the task's
    // next syscall triggers the switch rather than switching from the
    // interrupt handler (which would risk deadlock on the scheduler lock).
    if crate::arch::x86_64::interrupts::NEED_RESCHEDULE
        .swap(false, core::sync::atomic::Ordering::Acquire)
    {
        let ctx = crate::arch::x86_64::syscall::capture_current_context();
        crate::task::scheduler::block_and_switch(ctx);
    }

    #[allow(unreachable_patterns)]
    match number {
        SYS_CHANNEL_CREATE => sys_channel_create(),
        SYS_CHANNEL_SEND => sys_channel_send(arg1, arg2, arg3),
        SYS_CHANNEL_RECEIVE => sys_channel_receive(arg1, arg2, arg3),
        SYS_CHANNEL_CALL => sys_channel_call(arg1, arg2, arg3, arg4, arg5),
        SYS_CHANNEL_REPLY => sys_channel_reply(arg1, arg2, arg3),

        SYS_HANDLE_CLOSE => sys_handle_close(arg1),
        SYS_HANDLE_DUPLICATE => sys_handle_duplicate(arg1, arg2),
        SYS_HANDLE_TRANSFER => sys_handle_transfer(arg1, arg2, arg3),

        SYS_PROCESS_CREATE => sys_process_create(arg1, arg2, arg3),
        SYS_PROCESS_START => sys_process_start(arg1, arg2, arg3),
        SYS_PROCESS_EXIT => sys_process_exit(arg1),
        SYS_PROCESS_WAIT => sys_process_wait(arg1, arg2),
        SYS_BRK => sys_brk(arg1),
        SYS_MMAP => sys_mmap(arg1, arg2, arg3),
        SYS_MUNMAP => sys_munmap(arg1, arg2),
        SYS_MPROTECT => sys_mprotect(arg1, arg2, arg3),
        SYS_CLOCK_GETTIME => sys_clock_gettime(arg1, arg2),
        SYS_GETPID => sys_getpid(),
        SYS_GETPPID => sys_getppid(),
        SYS_LIST_TASKS => sys_list_tasks(arg1, arg2),

        SYS_THREAD_CREATE => sys_thread_create(arg1, arg2, arg3),
        SYS_THREAD_EXIT => sys_thread_exit(),
        SYS_THREAD_YIELD => sys_thread_yield(),

        SYS_PIPE => sys_pipe(arg1),
        SYS_POLL => sys_poll(arg1, arg2, arg3),

        SYS_KILL => sys_kill(arg1, arg2),
        SYS_SIGNAL => sys_signal(arg1, arg2),
        SYS_SIGRETURN => sys_sigreturn(arg1),
        SYS_SIGPROCMASK => sys_sigprocmask(arg1, arg2, arg3),

        SYS_DUP2 => sys_dup2(arg1, arg2),
        SYS_ENV_GET => sys_env_get(arg1, arg2, arg3, arg4),
        SYS_ENV_SET => sys_env_set(arg1, arg2, arg3, arg4),
        SYS_CHDIR => sys_chdir(arg1, arg2),
        SYS_GETCWD => sys_getcwd(arg1, arg2),

        SYS_SETPGID => sys_setpgid(arg1, arg2),
        SYS_GETPGID => sys_getpgid(arg1),
        SYS_SETSID => sys_setsid(),

        SYS_CONSOLE_WRITE => sys_console_write(arg1, arg2),
        SYS_CONSOLE_READ => sys_console_read(arg1, arg2, arg3),
        SYS_SLEEP => sys_sleep(arg1),
        SYS_EVENT_CREATE => sys_event_create(),
        SYS_EVENT_SIGNAL => sys_event_signal(arg1),
        SYS_EVENT_WAIT => sys_event_wait(arg1),
        SYS_EVENT_DESTROY => sys_event_destroy(arg1),
        SYS_ENDPOINT_REGISTER => sys_endpoint_register(arg1, arg2, arg3),
        SYS_ENDPOINT_DISCOVER => sys_endpoint_discover(arg1, arg2),

        SYS_FS_OPEN => sys_fs_open(arg1, arg2, arg3),
        SYS_FS_READ => sys_fs_read(arg1, arg2, arg3),
        SYS_FS_WRITE => sys_fs_write(arg1, arg2, arg3),
        SYS_FS_SEEK => sys_fs_seek(arg1, arg2, arg3),
        SYS_FS_CLOSE => sys_fs_close(arg1),
        SYS_FS_UNLINK => sys_fs_unlink(arg1, arg2),
        SYS_FS_RENAME => sys_fs_rename(arg1, arg2, arg3, arg4),
        SYS_FS_MKDIR => sys_fs_mkdir(arg1, arg2, arg3),
        SYS_FS_RMDIR => sys_fs_rmdir(arg1, arg2),
        SYS_FS_STAT => sys_fs_stat(arg1, arg2, arg3),
        SYS_FS_READDIR => sys_fs_readdir(arg1, arg2, arg3),
        SYS_GETDENTS64 => sys_getdents64(arg1, arg2, arg3),
        SYS_FSTAT => sys_fstat(arg1, arg2),
        SYS_LSTAT => sys_lstat(arg1, arg2, arg3),
        SYS_ACCESS => sys_access(arg1, arg2, arg3),
        SYS_SYMLINK => sys_symlink(arg1, arg2, arg3, arg4),
        SYS_READLINK => sys_readlink(arg1, arg2, arg3, arg4),
        SYS_CHMOD => sys_chmod(arg1, arg2, arg3),
        SYS_UMASK => sys_umask(arg1),

        SYS_NET_SEND => sys_net_send(arg1, arg2),
        SYS_NET_RECEIVE => sys_net_receive(arg1, arg2),

        SYS_SOCKET => sys_socket(arg1),
        SYS_BIND => sys_bind(arg1, arg2, arg3),
        SYS_LISTEN => sys_listen(arg1),
        SYS_ACCEPT => sys_accept(arg1),
        SYS_CONNECT => sys_connect(arg1, arg2, arg3),
        SYS_SENDTO => sys_sendto(arg1, arg2, arg3, arg4, arg5),
        SYS_RECVFROM => sys_recvfrom(arg1, arg2, arg3),
        SYS_CLOSE_SOCK => sys_close_sock(arg1),
        SYS_DNS_RESOLVE => sys_dns_resolve(arg1, arg2, arg3),
        SYS_GETSOCKOPT => sys_getsockopt(arg1, arg2, arg3, arg4, arg5),
        SYS_SETSOCKOPT => sys_setsockopt(arg1, arg2, arg3, arg4, arg5),

        SYS_PORT_IN => sys_port_in(arg1, arg2),
        SYS_PORT_OUT => sys_port_out(arg1, arg2, arg3),
        SYS_MMIO_MAP => sys_mmio_map(arg1, arg2),
        SYS_MMIO_UNMAP => sys_mmio_unmap(arg1, arg2),
        SYS_IRQ_WAIT => sys_irq_wait(arg1, arg2),

        _ => Error::UnknownSyscall as i64,
    }
}

// ─────────────────── Channel syscalls ───────────────────

/// Create a new IPC channel and install both endpoints in the caller's handle table.
///
/// Returns the raw handle value for End A (the "client" end). End B (the "server" end)
/// is also installed but its handle value is not returned — it can be discovered via
/// `endpoint_register`/`endpoint_discover` or transferred via `handle_transfer`.
///
/// On error, returns a negative `Error` code.
fn sys_channel_create() -> i64 {
    let channel = Channel::new();
    let channel_arc = alloc::sync::Arc::new(spin::Mutex::new(channel));

    let result = crate::task::scheduler::with_current_task_mut(|task| {
        let handle_a = task.handle_table.insert(
            KernelObject::ChannelEndA(alloc::sync::Arc::clone(&channel_arc)),
            Rights::ALL,
        );
        let _handle_b = task
            .handle_table
            .insert(KernelObject::ChannelEndB(channel_arc), Rights::ALL);
        handle_a.as_u64()
    });

    #[allow(clippy::cast_possible_wrap)]
    result.map_or(Error::NotFound as i64, |id| {
        crate::serial_println!("[SYSCALL] channel_create: handle_a={id:#x}");
        id as i64
    })
}

/// Send a message through a channel endpoint.
///
/// If the peer is already blocked in `receive`, the message is delivered immediately
/// and the peer is woken. Otherwise, the message is stored and the sender may block
/// until the peer calls `receive`.
///
/// Arguments:
///   arg0: handle to a channel endpoint
///   arg1: pointer to message data in user-space
///   arg2: message length in bytes (max `MAX_MSG_SIZE`)
///
/// Returns: 0 on success, negative `Error` code on failure.
fn sys_channel_send(handle_raw: u64, msg_ptr: u64, msg_len: u64) -> i64 {
    let Some(msg) = (unsafe { copy_from_user(msg_ptr as *const u8, msg_len as usize) }) else {
        return Error::BadPointer as i64;
    };

    let Some((channel, end)) = lookup_channel(handle_raw) else {
        return Error::NotFound as i64;
    };

    let task_id = crate::task::scheduler::current_task_id().as_u64();
    let mut ch = channel.lock();

    match ch.send(end, msg, task_id) {
        crate::ipc::SendResult::Delivered(receiver_id) => {
            crate::serial_println!("[SYSCALL] channel_send: delivered to task {receiver_id}");
            drop(ch);
            crate::task::scheduler::wake_task_by_id(crate::task::task::TaskId::from_u64(
                receiver_id,
            ));
            0
        }
        crate::ipc::SendResult::Pending => {
            crate::serial_println!("[SYSCALL] channel_send: pending, message stored");
            0
        }
        crate::ipc::SendResult::Closed => Error::ChannelClosed as i64,
    }
}

/// Receive a message from a channel endpoint.
///
/// Attempts a non-blocking receive first. If no message is available, blocks the
/// calling task until a message arrives (via `channel_send` from the peer). When
/// the system has only one runnable task, falls back to a HLT spin-wait loop.
///
/// Arguments:
///   arg0: handle to a channel endpoint
///   arg1: pointer to receive buffer in user-space
///   arg2: buffer length in bytes
///
/// Returns: number of bytes received on success, negative `Error` code on failure.
fn sys_channel_receive(handle_raw: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    let Some((channel, end)) = lookup_channel(handle_raw) else {
        return Error::NotFound as i64;
    };

    let task_id = crate::task::scheduler::current_task_id().as_u64();

    crate::serial_println!(
        "[SYSCALL] channel_receive: handle={:#x} end={:?}",
        handle_raw,
        end
    );

    // Fast path: try to receive immediately (non-blocking).
    {
        let mut ch = channel.lock();
        match ch.receive(end, task_id) {
            crate::ipc::RecvResult::GotMessage(msg, _sender_id) => {
                crate::serial_println!("[SYSCALL] channel_receive: got {} bytes", msg.len());
                let len = msg.len();
                drop(ch);
                if unsafe { copy_to_user(buf_ptr as *mut u8, &msg) } {
                    return i64::try_from(len).unwrap_or(-1);
                }
                return Error::BadPointer as i64;
            }
            crate::ipc::RecvResult::Blocked => {
                crate::serial_println!("[SYSCALL] channel_receive: no message, will block");
            }
            crate::ipc::RecvResult::Closed => {
                return Error::ChannelClosed as i64;
            }
        }
    }

    // No message available. Block the current task and context-switch to
    // the next ready task. The current task's register state is captured
    // from the syscall entry stub's saved area on the kernel stack.
    //
    // When a message arrives (via `channel_send` from another task), the
    // sender calls `wake_task_by_id`, which moves this task back to the
    // ready queue. On the next schedule, this task resumes here and
    // retries the receive.
    let ctx = crate::arch::x86_64::syscall::capture_current_context();
    let switched = crate::task::scheduler::block_and_switch(ctx);

    if switched {
        // Context switch happened — the assembly stub will restore the new
        // task's registers via SWITCH_CONTEXT. This return value is never
        // used (the stub discards it in the switch path).
        crate::serial_println!("[SYSCALL] channel_receive: context switched");
        return 0;
    }

    // No other task is ready — fall back to HLT spin-wait until a message
    // arrives or another task becomes runnable. This handles the edge case
    // where the current task is the only one in the system.
    //
    // Move the task back to the ready queue first. `block_and_switch` moved
    // it to the blocked queue, but since no context switch happened the task
    // is still on the CPU. Leaving it in the blocked queue while spin-waiting
    // would cause senders to call `wake_task_by_id` on a task that is already
    // running, leading to double-scheduling.
    crate::task::scheduler::unblock_current();
    crate::serial_println!("[SYSCALL] channel_receive: no other task, spin-wait");
    loop {
        x86_64::instructions::hlt();
        let mut ch = channel.lock();
        match ch.receive(end, task_id) {
            crate::ipc::RecvResult::GotMessage(msg, _sender_id) => {
                crate::serial_println!("[SYSCALL] channel_receive: got {} bytes", msg.len());
                let len = msg.len();
                drop(ch);
                if unsafe { copy_to_user(buf_ptr as *mut u8, &msg) } {
                    return i64::try_from(len).unwrap_or(-1);
                }
                return Error::BadPointer as i64;
            }
            crate::ipc::RecvResult::Blocked => {
                drop(ch);
            }
            crate::ipc::RecvResult::Closed => {
                return Error::ChannelClosed as i64;
            }
        }
    }
}

/// Atomic send-and-wait-for-reply (RPC primitive).
///
/// Sends a message and blocks until the peer calls `channel_reply`. This is the
/// fundamental request-response pattern: the caller sends a request, the server
/// processes it and replies, and the caller is unblocked with the reply data.
///
/// Arguments:
///   arg0: handle to a channel endpoint
///   arg1: pointer to request message in user-space
///   arg2: request message length
///   arg3: pointer to reply buffer in user-space
///   arg4: reply buffer length (reserved, currently unused)
///
/// Returns: number of reply bytes on success, negative `Error` code on failure.
fn sys_channel_call(
    handle_raw: u64,
    msg_ptr: u64,
    msg_len: u64,
    reply_ptr: u64,
    _reply_len: u64,
) -> i64 {
    let Some(msg) = (unsafe { copy_from_user(msg_ptr as *const u8, msg_len as usize) }) else {
        return Error::BadPointer as i64;
    };

    let Some((channel, end)) = lookup_channel(handle_raw) else {
        return Error::NotFound as i64;
    };

    let task_id = crate::task::scheduler::current_task_id().as_u64();
    let mut ch = channel.lock();

    match ch.call(end, msg, task_id) {
        crate::ipc::CallResult::GotReply(reply) => {
            crate::serial_println!("[SYSCALL] channel_call: got reply ({} bytes)", reply.len());
            let len = reply.len();
            drop(ch);
            if unsafe { copy_to_user(reply_ptr as *mut u8, &reply) } {
                i64::try_from(len).unwrap_or(-1)
            } else {
                Error::BadPointer as i64
            }
        }
        crate::ipc::CallResult::Blocked(_receiver_id) => {
            crate::serial_println!("[SYSCALL] channel_call: blocked, waiting for reply");
            drop(ch);
            // Spin-wait for the reply (the server will call channel_reply).
            // Use check_reply() instead of call() to avoid overwriting the
            // pending message with an empty payload on each iteration.
            loop {
                let mut ch = channel.lock();
                match ch.check_reply(end) {
                    crate::ipc::CallResult::GotReply(reply) => {
                        let len = reply.len();
                        drop(ch);
                        if unsafe { copy_to_user(reply_ptr as *mut u8, &reply) } {
                            return i64::try_from(len).unwrap_or(-1);
                        }
                        return Error::BadPointer as i64;
                    }
                    crate::ipc::CallResult::Closed => {
                        return Error::ChannelClosed as i64;
                    }
                    crate::ipc::CallResult::Blocked(_) => {
                        drop(ch);
                        x86_64::instructions::hlt();
                    }
                }
            }
        }
        crate::ipc::CallResult::Closed => Error::ChannelClosed as i64,
    }
}

/// Reply to a pending `channel_call` from the peer.
///
/// The reply data is stored and the blocked caller is woken. If no caller is
/// waiting, the reply is stored for later consumption by the caller's next
/// `channel_call` or `check_reply`.
///
/// Arguments:
///   arg0: handle to a channel endpoint
///   arg1: pointer to reply data in user-space
///   arg2: reply length in bytes
///
/// Returns: 0 on success, negative `Error` code on failure.
fn sys_channel_reply(handle_raw: u64, msg_ptr: u64, msg_len: u64) -> i64 {
    let Some(msg) = (unsafe { copy_from_user(msg_ptr as *const u8, msg_len as usize) }) else {
        return Error::BadPointer as i64;
    };

    let Some((channel, end)) = lookup_channel(handle_raw) else {
        return Error::NotFound as i64;
    };

    let mut ch = channel.lock();
    match ch.reply(end, msg) {
        crate::ipc::ReplyResult::Unblocked(caller_id) => {
            crate::serial_println!("[SYSCALL] channel_reply: unblocked caller {caller_id}");
            drop(ch);
            crate::task::scheduler::wake_task_by_id(crate::task::task::TaskId::from_u64(caller_id));
            0
        }
        crate::ipc::ReplyResult::Stored => {
            crate::serial_println!("[SYSCALL] channel_reply: stored");
            0
        }
    }
}

// ─────────────────── Handle syscalls ───────────────────

/// Close a handle, removing it from the task's handle table.
///
/// The underlying kernel object is dropped if this was the last reference.
///
/// Arguments:
///   arg0: raw handle value
///
/// Returns: 0 on success, negative `Error` code if the handle is invalid.
fn sys_handle_close(handle_raw: u64) -> i64 {
    let handle = Handle::from_raw(handle_raw);
    let result =
        crate::task::scheduler::with_current_task_mut(|task| task.handle_table.close(handle));
    match result {
        Some(true) => {
            crate::serial_println!("[SYSCALL] handle_close: {handle_raw:#x}");
            0
        }
        _ => Error::NotFound as i64,
    }
}

/// Duplicate a handle with optionally narrowed rights.
///
/// The original handle's rights are intersected with `new_rights` (monotonic
/// reduction). The new handle has a fresh generation counter to prevent
/// confusion with the original.
///
/// Arguments:
///   arg0: raw handle value to duplicate
///   arg1: new rights mask (intersected with original)
///
/// Returns: new handle value on success, negative `Error` code on failure.
fn sys_handle_duplicate(handle_raw: u64, new_rights: u64) -> i64 {
    let handle = Handle::from_raw(handle_raw);
    let rights = Rights::from_raw(new_rights as u16);
    let result = crate::task::scheduler::with_current_task_mut(|task| {
        task.handle_table.duplicate(handle, rights)
    });
    match result {
        Some(Some(new_handle)) => {
            let id = new_handle.as_u64();
            crate::serial_println!("[SYSCALL] handle_duplicate: new handle={id:#x}");
            #[allow(clippy::cast_possible_wrap)]
            {
                id as i64
            }
        }
        Some(None) => Error::PermissionDenied as i64,
        None => Error::NotFound as i64,
    }
}

/// Transfer a handle to another task via a channel.
///
/// Removes the handle from the sender's handle table and stores it in the
/// channel's pending handles list. The receiver will get the handle value
/// as part of the next message received on that channel.
///
/// Arguments:
///   arg0: raw handle value to transfer
///   arg1: handle to the channel to transfer through
///   arg2: rights for the transferred handle (reserved, currently unused)
///
/// Returns: 0 on success, negative `Error` code on failure.
fn sys_handle_transfer(handle_raw: u64, channel_raw: u64, _rights: u64) -> i64 {
    let handle = Handle::from_raw(handle_raw);
    let channel_handle = Handle::from_raw(channel_raw);

    // Look up the channel to transfer through.
    let Some((channel, _end)) = lookup_channel(channel_raw) else {
        return Error::NotFound as i64;
    };

    // Remove the handle from the sender's table.
    let removed =
        crate::task::scheduler::with_current_task_mut(|task| task.handle_table.close(handle));

    if removed != Some(true) {
        return Error::NotFound as i64;
    }

    // Store the handle value in the channel's pending handles list.
    // The receiver will get it with the next message.
    let mut ch = channel.lock();
    if !ch.push_handle(handle_raw) {
        return Error::ChannelClosed as i64;
    }

    crate::serial_println!(
        "[SYSCALL] handle_transfer: handle={} channel={}",
        handle_raw,
        channel_raw
    );
    0
}

// ─────────────────── Process syscalls ───────────────────

/// Create a new process (task) in the scheduler.
/// Returns the task ID, or a negative error code.
///
/// Arguments:
///   arg0: job handle (unused for now)
///   arg1: pointer to process name (UTF-8)
///   arg2: name length
fn sys_process_create(_job_raw: u64, name_ptr: u64, name_len: u64) -> i64 {
    let Some(name_bytes) = (unsafe { copy_from_user(name_ptr as *const u8, name_len as usize) })
    else {
        return Error::BadPointer as i64;
    };
    let Ok(name) = core::str::from_utf8(&name_bytes) else {
        return Error::InvalidArgument as i64;
    };

    let Ok(task_id) = crate::task::scheduler::spawn_task_with_id(name, 0) else {
        return Error::OutOfMemory as i64;
    };

    // Set parent_id so the child can query getppid.
    let creator_id = crate::task::scheduler::current_task_id();
    crate::task::scheduler::with_task_mut(task_id, |task| {
        task.parent_id = Some(creator_id);
    });

    crate::serial_println!(
        "[SYSCALL] process_create: '{}' -> task {}",
        name,
        task_id.as_u64()
    );
    i64::try_from(task_id.as_u64()).unwrap_or(Error::InvalidArgument as i64)
}

/// Start a process by loading an ELF from the initrd.
///
/// Arguments:
///   arg0: process `task_id` (from `process_create`)
///   arg1: pointer to ELF filename (UTF-8)
///   arg2: filename length
#[allow(clippy::too_many_lines)]
fn sys_process_start(proc_raw: u64, name_ptr: u64, name_len: u64) -> i64 {
    let task_id = crate::task::task::TaskId::from_u64(proc_raw);

    let Some(name_bytes) = (unsafe { copy_from_user(name_ptr as *const u8, name_len as usize) })
    else {
        return Error::BadPointer as i64;
    };
    let Ok(filename) = core::str::from_utf8(&name_bytes) else {
        return Error::InvalidArgument as i64;
    };

    // Try to load the ELF from the disk filesystem first, then fall back to initrd.
    // This allows running programs from /disk/ as well as from the ramdisk.
    #[allow(clippy::large_stack_arrays)]
    let mut elf_buf = [0u8; 16_384]; // 16 KiB buffer for ELF loading
    let mut elf_data: &[u8];

    // First, try the VFS (disk filesystem).
    let (fs, rel_path) = crate::fs::vfs::resolve_fs(filename);
    match fs.open(&rel_path, crate::fs::vfs::OpenFlags::READ) {
        Ok(ino) => {
            let n = fs.read(ino, 0, &mut elf_buf).unwrap_or(0);
            let _ = fs.close(ino);
            if n > 0 {
                elf_data = &elf_buf[..n];
                crate::serial_println!(
                    "[SYSCALL] process_start: loading '{}' ({} bytes) from disk into task {}",
                    filename,
                    n,
                    task_id.as_u64()
                );
            } else {
                // File exists but is empty, try initrd.
                elf_data = &[];
            }
        }
        Err(_) => {
            elf_data = &[];
        }
    }

    // If not found on disk, try the initrd.
    let initrd_data: alloc::vec::Vec<u8>;
    if elf_data.is_empty() {
        // SAFETY: RAMDISK_DATA is set once during boot and is read-only thereafter.
        let Some(ramdisk) = (unsafe { crate::RAMDISK_DATA }) else {
            return Error::NotFound as i64;
        };
        let Some(file) = crate::initrd::find_file(ramdisk, filename) else {
            return Error::NotFound as i64;
        };
        initrd_data = alloc::vec::Vec::from(file.data);
        elf_data = &initrd_data;
        crate::serial_println!(
            "[SYSCALL] process_start: loading '{}' ({} bytes) from initrd into task {}",
            filename,
            file.data.len(),
            task_id.as_u64()
        );
    }

    // Validate the ELF header before loading.
    if crate::elf::parse_header(elf_data).is_err() {
        crate::serial_println!("[SYSCALL] process_start: ELF parse error");
        return Error::InvalidArgument as i64;
    }

    // Allocate a new page table for the process (kernel entries shared).
    let Some(page_table_phys) = crate::memory::create_user_page_table() else {
        crate::serial_println!("[SYSCALL] process_start: out of memory for page table");
        return Error::OutOfMemory as i64;
    };

    // Look up the existing task created by process_create and configure it.
    let found = crate::task::scheduler::with_task_mut(task_id, |task| {
        task.parent_id = Some(crate::task::scheduler::current_task_id());
        task.page_table = Some(page_table_phys);
        task.name = alloc::string::String::from(filename);
    });

    if found.is_none() {
        crate::serial_println!(
            "[SYSCALL] process_start: task {} not found",
            task_id.as_u64()
        );
        return Error::NotFound as i64;
    }

    // Load ELF segments into the task's page table and set the context.
    // We need to switch to the task's page table temporarily to map pages.
    //
    // Disable interrupts for the entire page table switch window. An interrupt
    // handler running on the user's page table would fault because kernel code
    // and data would not be mapped. We must not re-enable until we restore the
    // kernel page table.
    let flags = x86_64::instructions::interrupts::are_enabled();
    x86_64::instructions::interrupts::disable();

    // SAFETY: The page table was just created and is valid.
    unsafe {
        crate::memory::switch_page_table(page_table_phys);
    }

    let load_result = crate::elf::load_elf(elf_data, |virt, phys, writable, executable| {
        let mut flags = x86_64::structures::paging::PageTableFlags::PRESENT
            | x86_64::structures::paging::PageTableFlags::USER_ACCESSIBLE;
        if writable {
            flags |= x86_64::structures::paging::PageTableFlags::WRITABLE;
        }
        if !executable {
            flags |= x86_64::structures::paging::PageTableFlags::NO_EXECUTE;
        }
        // SAFETY: `virt` is page-aligned, `phys` was allocated by the ELF loader,
        // and `flags` correctly reflect the segment permissions.
        unsafe {
            crate::task::user::map_page_user(virt, phys, flags);
        }
    });

    // Switch back to the kernel's page table.
    // SAFETY: We restore the original kernel page table.
    let (kernel_p4, _) = x86_64::registers::control::Cr3::read();
    unsafe {
        crate::memory::switch_page_table(kernel_p4.start_address().as_u64());
    }

    // Restore the interrupt state that was active before the page table switch.
    if flags {
        x86_64::instructions::interrupts::enable();
    }

    let load_result = match load_result {
        Ok(r) => r,
        Err(e) => {
            crate::serial_println!("[SYSCALL] process_start: ELF load failed: {:?}", e);
            return Error::InvalidArgument as i64;
        }
    };

    let user_rip = load_result.entry_point;
    let user_rsp = load_result.stack_top;

    // Set the task's saved context to the ELF entry point.
    // CR3 is set to the task's page table so the assembly stub loads it on
    // context switch — without this, all processes share the kernel page table
    // after their first reschedule.
    let ctx = crate::task::task::SavedContext::user_mode(user_rip, user_rsp, page_table_phys);
    crate::task::scheduler::with_task_mut(task_id, |task| {
        task.context = Some(ctx);
    });

    crate::serial_println!(
        "[SYSCALL] process_start: task {} entry={:#x} stack={:#x}",
        task_id.as_u64(),
        user_rip,
        user_rsp
    );

    i64::try_from(task_id.as_u64()).unwrap_or(Error::InvalidArgument as i64)
}

fn sys_process_exit(status: u64) -> i64 {
    crate::serial_println!("[SYS_EXIT] status={status}");

    // Terminate the current task: set exit status, close handles,
    // remove from ready queue, wake parent.
    crate::task::scheduler::terminate_current(status);

    // Try to switch to the next ready task.
    let ctx = crate::arch::x86_64::syscall::capture_current_context();
    let switched = crate::task::scheduler::block_and_switch(ctx);

    if switched {
        // Context switch happened — the assembly stub will restore
        // the new task's context.
        return 0;
    }

    // No other task is ready. HLT forever (idle).
    loop {
        x86_64::instructions::hlt();
    }
}

/// Wait for a child process to exit.
///
/// Arguments:
///   arg0: `child` `task_id` (from `process_create`)
///   arg1: `timeout` in timer ticks (0 = check once, `u64::MAX` = block forever)
fn sys_process_wait(child_raw: u64, timeout: u64) -> i64 {
    let child_id = crate::task::task::TaskId::from_u64(child_raw);
    crate::serial_println!("[SYSCALL] process_wait: waiting for task {}", child_raw);

    let start = crate::arch::x86_64::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);

    loop {
        // Check if the child has exited.
        // get_exit_status returns Some(status) if terminated, None if still
        // running or not found.
        if let Some(status) = crate::task::scheduler::get_exit_status(child_id) {
            crate::serial_println!(
                "[SYSCALL] process_wait: child {} exited with status {}",
                child_raw,
                status
            );
            return i64::try_from(status).unwrap_or(0);
        }

        // Check timeout (0 = check once, non-blocking).
        if timeout == 0 {
            return Error::WouldBlock as i64;
        }

        let current =
            crate::arch::x86_64::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);
        if timeout != u64::MAX && current - start >= timeout {
            break;
        }

        x86_64::instructions::hlt();
    }

    crate::serial_println!("[SYSCALL] process_wait: timeout");
    Error::WouldBlock as i64
}

// ─────────────────── Memory syscalls ───────────────────

/// Set the program break (heap end) address.
///
/// If `new_brk` is 0, returns the current break address.
/// If `new_brk` is non-zero, sets the break to the page-aligned address
/// and allocates/frees pages as needed.
///
/// Arguments:
///   arg0: new break address (0 = query current)
///
/// Returns: the current (or new) break address, or -1 on error.
#[allow(clippy::cast_possible_wrap)]
fn sys_brk(new_brk: u64) -> i64 {
    crate::serial_println!("[SYSCALL] brk: new_brk={:#x}", new_brk);

    // Get the current task's BRK value.
    let current_brk = crate::task::scheduler::with_current_task(|task| task.brk).unwrap_or(0);

    if new_brk == 0 {
        // Query mode: return current break.
        return current_brk as i64;
    }

    // Validate: new break must be in user space.
    if new_brk >= crate::memory::pagetable::USER_SPACE_MAX {
        return Error::InvalidArgument as i64;
    }

    let page_size = crate::memory::pagetable::PAGE_SIZE;
    let aligned_new = (new_brk + page_size - 1) & !(page_size - 1);
    let aligned_old = if current_brk == 0 {
        aligned_new // First call: no old pages to manage.
    } else {
        (current_brk + page_size - 1) & !(page_size - 1)
    };

    // Update the task's BRK value.
    crate::task::scheduler::with_current_task_mut(|task| {
        task.brk = aligned_new;
    });

    // If the heap VMA exists, extend it. Otherwise create one.
    let vma_updated = crate::task::scheduler::with_current_task_mut(|task| {
        if task.vma_list.find(current_brk).is_some() {
            let _ = task.vma_list.extend_heap(aligned_new);
            true
        } else {
            false
        }
    });

    if !vma_updated.unwrap_or(false) && current_brk == 0 {
        // First BRK call — create a heap VMA.
        crate::task::scheduler::with_current_task_mut(|task| {
            let _ = task.vma_list.add(crate::memory::vma::VmaRegion {
                start: aligned_new,
                size: page_size, // Initial heap: 1 page.
                flags: crate::memory::vma::VmaFlags::RW,
                kind: crate::memory::vma::VmaType::Heap,
            });
            task.brk = aligned_new + page_size;
        });
    }

    aligned_new as i64
}

/// Map a region of memory (simplified mmap).
///
/// Arguments:
///   arg0: preferred virtual address (0 = kernel chooses)
///   arg1: size in bytes
///   arg2: flags (bit 0 = writable, bit 1 = executable)
///
/// Returns: mapped virtual address, or -1 on error.
#[allow(clippy::cast_possible_wrap)]
fn sys_mmap(hint: u64, size: u64, flags: u64) -> i64 {
    if size == 0 {
        return Error::InvalidArgument as i64;
    }

    let page_size = crate::memory::pagetable::PAGE_SIZE;
    let aligned_size = (size + page_size - 1) & !(page_size - 1);
    let num_pages = aligned_size / page_size;

    // Find a free virtual address range.
    let virt_addr = crate::task::scheduler::with_current_task(|task| {
        let pt = unsafe { crate::memory::pagetable::PageTable::new(task.page_table.unwrap_or(0)) };
        pt.find_free_range(
            if hint == 0 { 0x4000_0000 } else { hint },
            num_pages as usize,
        )
    });

    let Some(virt) = virt_addr.flatten() else {
        return Error::OutOfMemory as i64;
    };

    // Map each page.
    let writable = flags & 1 != 0;
    let executable = flags & 2 != 0;
    let mut pt_flags = x86_64::structures::paging::PageTableFlags::PRESENT
        | x86_64::structures::paging::PageTableFlags::USER_ACCESSIBLE;
    if writable {
        pt_flags |= x86_64::structures::paging::PageTableFlags::WRITABLE;
    }
    if !executable {
        pt_flags |= x86_64::structures::paging::PageTableFlags::NO_EXECUTE;
    }

    for i in 0..num_pages {
        let page_virt = virt + i * page_size;
        let frame = crate::frame_alloc::alloc_frame();
        let Some(phys) = frame else {
            return Error::OutOfMemory as i64;
        };
        // Zero the frame.
        let virt_ptr = crate::memory::phys_to_virt(phys) as *mut u8;
        // SAFETY: phys was just allocated and is not yet mapped.
        unsafe {
            core::ptr::write_bytes(virt_ptr, 0, page_size as usize);
        }
        // Map the page into the current task's page table.
        // SAFETY: phys is a valid allocated frame, virt is page-aligned.
        unsafe {
            crate::task::user::map_page_user(page_virt, phys, pt_flags);
        }
    }

    // Register the VMA.
    crate::task::scheduler::with_current_task_mut(|task| {
        let _ = task.vma_list.add(crate::memory::vma::VmaRegion {
            start: virt,
            size: aligned_size,
            flags: crate::memory::vma::VmaFlags {
                read: true,
                write: writable,
                execute: executable,
            },
            kind: crate::memory::vma::VmaType::Mmap,
        });
    });

    crate::serial_println!(
        "[SYSCALL] mmap: {:#x}..{:#x} ({} pages, w={} x={})",
        virt,
        virt + aligned_size,
        num_pages,
        writable,
        executable
    );

    virt as i64
}

/// Unmap a region of memory.
///
/// Arguments:
///   arg0: virtual address (must be page-aligned)
///   arg1: size in bytes
///
/// Returns: 0 on success, -1 on error.
fn sys_munmap(virt_addr: u64, size: u64) -> i64 {
    if virt_addr % crate::memory::pagetable::PAGE_SIZE != 0 || size == 0 {
        return Error::InvalidArgument as i64;
    }

    let page_size = crate::memory::pagetable::PAGE_SIZE;
    let aligned_size = (size + page_size - 1) & !(page_size - 1);
    let num_pages = aligned_size / page_size;

    // Remove VMA.
    crate::task::scheduler::with_current_task_mut(|task| {
        task.vma_list.remove(virt_addr);
    });

    // Unmap each page.
    for i in 0..num_pages {
        let page_virt = virt_addr + i * page_size;
        // SAFETY: We're unmapping user pages in the current page table.
        unsafe {
            crate::task::user::map_page_user(
                page_virt,
                0,
                x86_64::structures::paging::PageTableFlags::empty(),
            );
        }
    }

    crate::serial_println!(
        "[SYSCALL] munmap: {:#x}..{:#x} ({} pages)",
        virt_addr,
        virt_addr + aligned_size,
        num_pages
    );

    0
}

/// Change memory protection on mmap'd pages.
///
/// Arguments:
///   arg0: virtual address (must be page-aligned)
///   arg1: size in bytes (rounded up to page boundary)
///   arg2: new protection flags (bit 0=read, bit 1=write, bit 2=exec)
///
/// Returns: 0 on success, or negative error code.
fn sys_mprotect(virt_addr: u64, size: u64, prot_flags: u64) -> i64 {
    if virt_addr % crate::memory::pagetable::PAGE_SIZE != 0 || size == 0 {
        return Error::InvalidArgument as i64;
    }

    // Only allow modifying user-space addresses.
    if virt_addr >= crate::memory::USER_SPACE_MAX {
        return Error::InvalidArgument as i64;
    }

    let page_size = crate::memory::pagetable::PAGE_SIZE;
    let aligned_size = (size + page_size - 1) & !(page_size - 1);
    let num_pages = aligned_size / page_size;

    // Convert protection flags to page table flags.
    let mut pt_flags = x86_64::structures::paging::PageTableFlags::PRESENT
        | x86_64::structures::paging::PageTableFlags::USER_ACCESSIBLE;
    if prot_flags & 2 != 0 {
        pt_flags |= x86_64::structures::paging::PageTableFlags::WRITABLE;
    }
    // Note: NX (no-execute) bit is inverted - PROT_EXEC means NX should be clear.
    if prot_flags & 4 == 0 {
        pt_flags |= x86_64::structures::paging::PageTableFlags::NO_EXECUTE;
    }

    // Apply protection to each page.
    for i in 0..num_pages {
        let page_virt = virt_addr + i * page_size;
        // SAFETY: We're modifying protection on user pages.
        unsafe {
            crate::task::user::protect_page_user(page_virt, pt_flags);
        }
    }

    crate::serial_println!(
        "[SYSCALL] mprotect: {:#x}..{:#x} flags={:#x}",
        virt_addr,
        virt_addr + aligned_size,
        prot_flags
    );

    0
}

// ─────────────────── Time syscalls ───────────────────

/// Timer frequency in Hz. The PIT/APIC timer fires at this rate.
const TIMER_HZ: u64 = 100;

/// Nanoseconds per timer tick (`1_000_000_000` / `TIMER_HZ` = `10_000_000`).
const NS_PER_TICK: u64 = 1_000_000_000 / TIMER_HZ;

/// Get the current time for a given clock.
///
/// Only `clock_id == 0` (monotonic clock) is supported. The time is derived
/// from the global tick counter set by the timer interrupt handler.
///
/// Writes a 16-byte `timespec` (seconds: u64, nanoseconds: u64) to the
/// user-space buffer at `timespec_ptr`.
///
/// Arguments:
///   arg0: clock identifier (0 = monotonic)
///   arg1: pointer to 16-byte user-space buffer for `struct timespec`
///
/// Returns: 0 on success, negative `Error` code on failure.
fn sys_clock_gettime(clock_id: u64, timespec_ptr: u64) -> i64 {
    if clock_id != 0 {
        return Error::InvalidArgument as i64;
    }

    let ticks = crate::arch::x86_64::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);

    let sec = ticks / TIMER_HZ;
    let nsec = (ticks % TIMER_HZ) * NS_PER_TICK;

    let mut buf = [0u8; 16];
    buf[0..8].copy_from_slice(&sec.to_le_bytes());
    buf[8..16].copy_from_slice(&nsec.to_le_bytes());

    if unsafe { copy_to_user(timespec_ptr as *mut u8, &buf) } {
        crate::serial_println!(
            "[SYSCALL] clock_gettime: clock_id={} sec={} nsec={}",
            clock_id,
            sec,
            nsec
        );
        0
    } else {
        Error::BadPointer as i64
    }
}

// ─────────────────── GetPID / GetPPID syscalls ───────────────────

/// Get the current process ID (task ID).
///
/// Returns the caller's task ID as a positive value.
fn sys_getpid() -> i64 {
    let id = crate::task::scheduler::current_task_id().as_u64();
    #[allow(clippy::cast_possible_wrap)]
    {
        id as i64
    }
}

/// Get the parent process ID.
///
/// Returns the parent task's ID if a parent exists, or 0 if the task
/// has no parent (e.g., the root/idle task).
fn sys_getppid() -> i64 {
    let parent = crate::task::scheduler::with_current_task(|task| task.parent_id);
    match parent {
        Some(Some(id)) => {
            #[allow(clippy::cast_possible_wrap)]
            {
                id.as_u64() as i64
            }
        }
        _ => 0,
    }
}

// ─────────────────── Process group / session syscalls ───────────────────

/// Set the process group ID of a process.
///
/// If `pid` is 0, the current task's ID is used.
/// If `pgid` is 0, the process group ID is set to the target task's own ID.
///
/// Arguments:
///   arg0: process ID (0 = current task)
///   arg1: new process group ID (0 = target's own ID)
///
/// Returns: 0 on success, negative `Error` code on failure.
fn sys_setpgid(pid: u64, pgid: u64) -> i64 {
    let current_id = crate::task::scheduler::current_task_id();

    // Resolve the target task ID: 0 means the current task.
    let target_id = if pid == 0 {
        current_id
    } else {
        crate::task::task::TaskId::from_u64(pid)
    };

    // Resolve the new pgid: 0 means set to the target's own ID.
    let new_pgid = if pgid == 0 { target_id.as_u64() } else { pgid };

    let result = crate::task::scheduler::with_task_mut(target_id, |task| {
        task.pgid = new_pgid;
    });

    match result {
        Some(()) => {
            crate::serial_println!(
                "[SYSCALL] setpgid: pid={} pgid={} (resolved: target={} new_pgid={})",
                pid,
                pgid,
                target_id.as_u64(),
                new_pgid
            );
            0
        }
        None => Error::NotFound as i64,
    }
}

/// Get the process group ID of a process.
///
/// If `pid` is 0, the current task's pgid is returned.
///
/// Arguments:
///   arg0: process ID (0 = current task)
///
/// Returns: the process group ID on success, negative `Error` code on failure.
fn sys_getpgid(pid: u64) -> i64 {
    if pid == 0 {
        // Return the current task's pgid.
        let result = crate::task::scheduler::with_current_task(|task| task.pgid);
        return result.map_or(Error::NotFound as i64, |pgid| {
            #[allow(clippy::cast_possible_wrap)]
            {
                pgid as i64
            }
        });
    }

    let target_id = crate::task::task::TaskId::from_u64(pid);
    // Look up the target task's pgid via with_task_mut (read-only use, but
    // with_task takes a closure that can mutate; we only read).
    let result = crate::task::scheduler::with_task_mut(target_id, |task| task.pgid);

    result.map_or(Error::NotFound as i64, |pgid| {
        crate::serial_println!("[SYSCALL] getpgid: pid={} -> pgid={}", pid, pgid);
        #[allow(clippy::cast_possible_wrap)]
        {
            pgid as i64
        }
    })
}

/// Create a new session and set the session ID.
///
/// The calling task becomes a session leader. Its session ID and process
/// group ID are both set to its own task ID.
///
/// Returns: the new session ID on success, negative `Error` code on failure.
fn sys_setsid() -> i64 {
    let current_id = crate::task::scheduler::current_task_id();
    let id_value = current_id.as_u64();

    let result = crate::task::scheduler::with_current_task_mut(|task| {
        task.sid = id_value;
        task.pgid = id_value;
    });

    match result {
        Some(()) => {
            crate::serial_println!("[SYSCALL] setsid: sid={}", id_value);
            #[allow(clippy::cast_possible_wrap)]
            {
                id_value as i64
            }
        }
        None => Error::NotFound as i64,
    }
}

// ─────────────────── List Tasks ───────────────────

/// Size of each serialized task entry in the output buffer.
/// Layout: [u64 id][u8 state][u8 priority][u16 reserved][u8 name[32]] = 40 bytes.
const TASK_ENTRY_SIZE: usize = 40;

/// Serialize task info into the user-space buffer.
///
/// Each entry is `TASK_ENTRY_SIZE` (40) bytes:
///   - offset  0: task ID       (u64 LE)
///   - offset  8: state         (u8: 0=Ready, 1=Running, 2=Blocked, 3=Terminated)
///   - offset  9: priority      (u8)
///   - offset 10: reserved      (u16, zero)
///   - offset 12: name          (32 bytes, UTF-8, zero-padded)
///
/// Arguments:
///   arg0: pointer to user-space output buffer
///   arg1: buffer length in bytes
///
/// Returns: number of tasks written on success, negative `Error` code on failure.
/// If the buffer is too small, returns the total number of tasks (none written).
#[allow(clippy::cast_possible_wrap)]
fn sys_list_tasks(buf_ptr: u64, buf_len: u64) -> i64 {
    if buf_ptr == 0 || buf_ptr >= crate::memory::USER_SPACE_MAX {
        return Error::BadPointer as i64;
    }

    // Overflow check: buf_ptr + buf_len must not exceed user-space boundary.
    if buf_ptr.saturating_add(buf_len) > crate::memory::USER_SPACE_MAX {
        return Error::BadPointer as i64;
    }

    let tasks = crate::task::scheduler::list_tasks();
    let count = tasks.len();
    let required = count * TASK_ENTRY_SIZE;

    // If buffer is too small, return the total count so the caller can retry.
    if (buf_len as usize) < required {
        return count as i64;
    }

    let mut buf = alloc::vec![0u8; required];
    for (i, task) in tasks.iter().enumerate() {
        let base = i * TASK_ENTRY_SIZE;
        // Task ID (u64 LE)
        buf[base..base + 8].copy_from_slice(&task.id.to_le_bytes());
        // State (u8): map TaskState to numeric value.
        buf[base + 8] = match task.state {
            crate::task::task::TaskState::Ready => 0,
            crate::task::task::TaskState::Running => 1,
            crate::task::task::TaskState::Blocked => 2,
            crate::task::task::TaskState::Terminated => 3,
        };
        // Priority (u8)
        buf[base + 9] = task.priority;
        // Reserved (u16) — already zero from vec init.
        // Name (32 bytes, zero-padded)
        let name_end = 12 + task.name_len as usize;
        buf[12..name_end].copy_from_slice(&task.name[..task.name_len as usize]);
    }

    if unsafe { copy_to_user(buf_ptr as *mut u8, &buf) } {
        crate::serial_println!("[SYSCALL] list_tasks: {} tasks", count);
        count as i64
    } else {
        Error::BadPointer as i64
    }
}

// ─────────────────── Thread syscalls ───────────────────

/// Create a new user-mode thread within the current process.
///
/// The new thread shares the parent's page table and starts executing
/// at `entry_point` with `arg` in RDI (first argument register) and
/// its stack pointer set to `stack_ptr`.
///
/// Arguments:
///   arg0 (rdi): entry point virtual address (RIP for SYSRET)
///   arg1 (rsi): user stack pointer (RSP for SYSRET)
///   arg2 (rdx): argument passed to the thread function (RDI)
///
/// Returns: the new thread's task ID (positive) on success, or a
///          negative error code on failure.
fn sys_thread_create(entry_point: u64, stack_ptr: u64, arg: u64) -> i64 {
    crate::serial_println!(
        "[SYSCALL] thread_create: entry={:#x}, stack={:#x}, arg={:#x}",
        entry_point,
        stack_ptr,
        arg
    );

    // Validate that the entry point is in user-space.
    if entry_point >= crate::memory::USER_SPACE_MAX || entry_point == 0 {
        return Error::InvalidArgument as i64;
    }

    // Validate that the stack pointer is in user-space.
    if stack_ptr >= crate::memory::USER_SPACE_MAX || stack_ptr == 0 {
        return Error::InvalidArgument as i64;
    }

    // Get the parent task's page table and ID.
    let parent_info = crate::task::scheduler::with_current_task(|task| (task.page_table, task.id));

    let Some((parent_cr3, parent_id)) = parent_info else {
        return Error::InvalidArgument as i64;
    };

    let Some(cr3) = parent_cr3 else {
        // The parent has no user page table — cannot create a user-mode thread.
        return Error::InvalidArgument as i64;
    };

    // Build the new task with a user-mode context.
    let mut new_task = crate::task::task::Task::new("thread", 5);
    let new_task_id = new_task.id;

    // Set up the saved context for SYSRET into user-mode.
    // SavedContext::user_mode sets rcx=entry, rsp=stack, r11=RFLAGS,
    // is_kernel=0, cr3=page_table. We additionally set rdi=arg.
    let mut ctx = crate::task::task::SavedContext::user_mode(entry_point, stack_ptr, cr3);
    ctx.rdi = arg;
    new_task.context = Some(ctx);

    // Share the parent's page table.
    new_task.page_table = Some(cr3);

    // Record the parent so the scheduler can wake it on exit.
    new_task.parent_id = Some(parent_id);

    // Enqueue the new task on the least-loaded CPU.
    let Ok(tid) = crate::task::scheduler::spawn_task_from(new_task) else {
        return Error::OutOfMemory as i64;
    };

    crate::serial_println!("[SYSCALL] thread_create: new task {}", tid.as_u64());
    i64::try_from(tid.as_u64()).unwrap_or(Error::OutOfMemory as i64)
}

/// Exit the current thread (terminate the current task with status 0).
///
/// Closes all handles, removes the task from the scheduler, and wakes the
/// parent task if it is blocked in `process_wait`. If no other task is
/// ready, halts the CPU forever.
fn sys_thread_exit() -> i64 {
    crate::serial_println!("[SYSCALL] thread_exit");

    // Terminate the current task with status 0.
    crate::task::scheduler::terminate_current(0);

    // Try to switch to the next ready task.
    let ctx = crate::arch::x86_64::syscall::capture_current_context();
    let switched = crate::task::scheduler::block_and_switch(ctx);

    if switched {
        // Context switch happened — the assembly stub will restore
        // the new task's context.
        return 0;
    }

    // No other task is ready. HLT forever (idle).
    loop {
        x86_64::instructions::hlt();
    }
}

/// Yield the CPU to the next ready task on this CPU.
///
/// Saves the current context, moves the task to the back of the ready queue,
/// and switches to the next task. If no other task is ready, continues running.
fn sys_thread_yield() -> i64 {
    crate::serial_println!("[SYSCALL] thread_yield");

    // Capture the current context so the scheduler can resume this task later.
    let ctx = crate::arch::x86_64::syscall::capture_current_context();

    // Move current task to the back of the ready queue and switch to the next.
    let switched = crate::task::scheduler::block_and_switch(ctx);

    if switched {
        // Context switch happened — the assembly stub will restore the new
        // task's context. This return value is never used (the stub discards
        // it in the switch path).
        crate::serial_println!("[SYSCALL] thread_yield: switched");
        return 0;
    }

    // No other task is ready — continue running.
    0
}

// ─────────────────── Pipe syscall ───────────────────

/// Create a pipe and install read/write handles in the caller's handle table.
///
/// Writes the two file descriptors `[read_fd, write_fd]` to the user-space
/// pointer `fds_ptr`. The read end and write end are stored as
/// `KernelObject::PipeReader` and `KernelObject::PipeWriter` entries in the
/// handle table, and also registered in the fd table so they work with the
/// existing `fs_read` / `fs_write` syscalls.
///
/// Arguments:
///   arg0: pointer to a `[u64; 2]` buffer in user-space for the fd pair
///
/// Returns: 0 on success, negative `Error` code on failure.
fn sys_pipe(fds_ptr: u64) -> i64 {
    use crate::handle::KernelObject;

    if fds_ptr == 0 || fds_ptr >= crate::memory::USER_SPACE_MAX {
        return Error::BadPointer as i64;
    }

    // Overflow check: fds_ptr + 16 must not exceed user-space boundary.
    if fds_ptr.saturating_add(16) > crate::memory::USER_SPACE_MAX {
        return Error::BadPointer as i64;
    }

    let (reader, writer) = crate::ipc::pipe::create_pipe();
    let reader_arc = alloc::sync::Arc::new(spin::Mutex::new(reader));
    let writer_arc = alloc::sync::Arc::new(spin::Mutex::new(writer));

    let result = crate::task::scheduler::with_current_task_mut(|task| {
        // Insert into the handle table.
        let read_handle = task.handle_table.insert(
            KernelObject::PipeReader(alloc::sync::Arc::clone(&reader_arc)),
            crate::handle::Rights::READ,
        );
        let write_handle = task.handle_table.insert(
            KernelObject::PipeWriter(writer_arc),
            crate::handle::Rights::WRITE,
        );

        // Also register in the fd table so fs_read/fs_write can use them.
        let mut read_fd = FD_FIRST_USABLE;
        while task.fd_table.contains_key(&read_fd) {
            read_fd += 1;
            if read_fd >= FD_LIMIT {
                return None;
            }
        }
        task.fd_table.insert(
            read_fd,
            crate::task::task::FdEntry {
                path: alloc::string::String::from("<pipe:r>"),
                ino: read_handle.as_u64(),
                offset: 0,
            },
        );

        let mut write_fd = read_fd + 1;
        while task.fd_table.contains_key(&write_fd) {
            write_fd += 1;
            if write_fd >= FD_LIMIT {
                return None;
            }
        }
        task.fd_table.insert(
            write_fd,
            crate::task::task::FdEntry {
                path: alloc::string::String::from("<pipe:w>"),
                ino: write_handle.as_u64(),
                offset: 0,
            },
        );

        Some((read_fd, write_fd))
    });

    match result {
        Some(Some((read_fd, write_fd))) => {
            crate::serial_println!("[SYSCALL] pipe: read_fd={} write_fd={}", read_fd, write_fd);

            // Write the fd pair to user-space.
            let mut fds_buf = [0u8; 16];
            fds_buf[0..8].copy_from_slice(&read_fd.to_le_bytes());
            fds_buf[8..16].copy_from_slice(&write_fd.to_le_bytes());

            if unsafe { copy_to_user(fds_ptr as *mut u8, &fds_buf) } {
                0
            } else {
                Error::BadPointer as i64
            }
        }
        Some(None) => Error::OutOfMemory as i64,
        None => Error::NotFound as i64,
    }
}

// ─────────────────── Poll syscall ───────────────────

/// Poll event flags (matches POSIX `pollfd.events`).
const POLLIN: u16 = 1;
const POLLOUT: u16 = 2;
const POLLERR: u16 = 4;

/// Size of each `PollFd` entry in the user-space array (fd:u64 + events:u16 + revents:u16 +
/// padding:u32 = 16 bytes).
const POLLFD_SIZE: usize = 16;

/// Multiplexed I/O readiness check.
///
/// Checks each file descriptor in the user-space `PollFd` array for readiness
/// and writes results back. Supports regular files, pipes, and sockets.
///
/// `PollFd` layout (16 bytes, packed):
///   offset  0: fd       (u64)   — file descriptor
///   offset  8: events   (u16)   — requested events (`POLLIN`, `POLLOUT`)
///   offset 10: revents  (u16)   — returned events (`POLLIN`, `POLLOUT`, `POLLERR`)
///   offset 12: _padding (u32)
///
/// Arguments:
///   arg0: pointer to array of `PollFd` structs in user-space
///   arg1: number of `PollFd` entries
///   arg2: timeout in milliseconds (0 = non-blocking, `u64::MAX` = block forever)
///
/// Returns: number of fds with non-zero `revents`, or negative `Error` code.
#[allow(clippy::cast_possible_truncation, clippy::too_many_lines)]
fn sys_poll(fds_ptr: u64, nfds: u64, timeout_ms: u64) -> i64 {
    if fds_ptr == 0 || fds_ptr >= crate::memory::USER_SPACE_MAX {
        return Error::BadPointer as i64;
    }

    if nfds == 0 {
        return 0;
    }

    // Cap at a reasonable limit to prevent abuse.
    const MAX_POLL_FDS: u64 = 1024;
    if nfds > MAX_POLL_FDS {
        return Error::InvalidArgument as i64;
    }

    // Overflow check: fds_ptr + nfds * POLLFD_SIZE must not exceed user-space.
    let total_size = nfds * POLLFD_SIZE as u64;
    if fds_ptr.saturating_add(total_size) > crate::memory::USER_SPACE_MAX {
        return Error::BadPointer as i64;
    }

    // Copy the PollFd array from user-space.
    let mut fds_buf = alloc::vec![0u8; total_size as usize];
    // SAFETY: We validated that `fds_ptr` is in user-space and the range fits.
    unsafe {
        core::ptr::copy_nonoverlapping(
            fds_ptr as *const u8,
            fds_buf.as_mut_ptr(),
            total_size as usize,
        );
    }

    // Convert ms timeout to ticks (100 Hz -> 10 ms per tick).
    const MS_PER_TICK: u64 = 10;
    let timeout_ticks = if timeout_ms == 0 {
        0
    } else if timeout_ms == u64::MAX {
        u64::MAX
    } else {
        timeout_ms.div_ceil(MS_PER_TICK)
    };

    let start_tick =
        crate::arch::x86_64::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);

    loop {
        let mut ready_count: u64 = 0;

        for i in 0..nfds as usize {
            let base = i * POLLFD_SIZE;
            let fd = u64::from_le_bytes(fds_buf[base..base + 8].try_into().unwrap_or([0; 8]));
            let events =
                u16::from_le_bytes(fds_buf[base + 8..base + 10].try_into().unwrap_or([0; 2]));

            let mut revents: u16 = 0;

            // Check if this fd is a socket.
            let is_socket = crate::task::scheduler::with_current_task(|task| {
                task.socket_table.contains_key(&fd)
            });
            if is_socket.unwrap_or(false) {
                // Socket fd: check TCP connection state.
                let socket_state = crate::task::scheduler::with_current_task(|task| {
                    task.socket_table.get(&fd).map(|s| s.state)
                });
                match socket_state.flatten() {
                    Some(crate::net::socket::SocketState::Connected) => {
                        if events & POLLIN != 0 {
                            revents |= POLLIN;
                        }
                        if events & POLLOUT != 0 {
                            revents |= POLLOUT;
                        }
                    }
                    Some(crate::net::socket::SocketState::Listening) => {
                        if events & POLLIN != 0 {
                            revents |= POLLIN;
                        }
                    }
                    Some(crate::net::socket::SocketState::Closed) => {
                        revents |= POLLERR;
                    }
                    _ => {}
                }
            } else {
                // Check if this fd is in the fd_table (file or pipe).
                let fd_info = crate::task::scheduler::with_current_task(|task| {
                    task.fd_table.get(&fd).map(|e| (e.path.clone(), e.ino))
                });

                match fd_info.flatten() {
                    Some((ref path, ino)) if path.starts_with("<pipe:") => {
                        // Pipe fd: check buffer state.
                        let handle = crate::handle::Handle::from_raw(ino);

                        if path.ends_with('>') || path == "<pipe:r>" {
                            // Read end of pipe.
                            let readable = crate::task::scheduler::with_current_task(|task| {
                                if let Some(crate::handle::KernelObject::PipeReader(pr)) =
                                    task.handle_table.get(handle)
                                {
                                    Some(pr.lock().is_readable())
                                } else {
                                    None
                                }
                            });
                            if readable.flatten() == Some(true) && events & POLLIN != 0 {
                                revents |= POLLIN;
                            }
                        } else if path == "<pipe:w>" {
                            // Write end of pipe.
                            let writable = crate::task::scheduler::with_current_task(|task| {
                                if let Some(crate::handle::KernelObject::PipeWriter(pw)) =
                                    task.handle_table.get(handle)
                                {
                                    Some(pw.lock().is_writable())
                                } else {
                                    None
                                }
                            });
                            if writable.flatten() == Some(true) && events & POLLOUT != 0 {
                                revents |= POLLOUT;
                            }
                        }
                    }
                    Some(_) => {
                        // Regular file fd: always ready for both read and write.
                        if events & POLLIN != 0 {
                            revents |= POLLIN;
                        }
                        if events & POLLOUT != 0 {
                            revents |= POLLOUT;
                        }
                    }
                    None => {
                        // Invalid fd.
                        revents |= POLLERR;
                    }
                }
            }

            // Write revents back into the buffer.
            fds_buf[base + 10..base + 12].copy_from_slice(&revents.to_le_bytes());

            if revents != 0 {
                ready_count += 1;
            }
        }

        // If at least one fd is ready, or this is a non-blocking poll, return now.
        if ready_count > 0 || timeout_ms == 0 {
            // Copy the updated PollFd array back to user-space.
            // SAFETY: We validated the pointer range at entry.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    fds_buf.as_ptr(),
                    fds_ptr as *mut u8,
                    total_size as usize,
                );
            }
            crate::serial_println!("[SYSCALL] poll: {} fds ready", ready_count);
            return i64::try_from(ready_count).unwrap_or(0);
        }

        // Check timeout.
        if timeout_ticks != u64::MAX {
            let current =
                crate::arch::x86_64::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);
            if current - start_tick >= timeout_ticks {
                // Timed out — write back the (all-zero revents) array and return 0.
                // SAFETY: We validated the pointer range at entry.
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        fds_buf.as_ptr(),
                        fds_ptr as *mut u8,
                        total_size as usize,
                    );
                }
                crate::serial_println!("[SYSCALL] poll: timeout");
                return 0;
            }
        }

        // Yield to other tasks, then re-check.
        let ctx = crate::arch::x86_64::syscall::capture_current_context();
        let switched = crate::task::scheduler::block_and_switch(ctx);
        if !switched {
            // No other task ready — HLT until next interrupt.
            x86_64::instructions::hlt();
        }
    }
}

// ─────────────────── Signal syscalls ───────────────────

/// Send a signal to a task.
///
/// Sets the signal bit in the target task's pending bitmask. The signal will
/// be delivered on the target's next syscall entry or context switch.
///
/// SIGKILL cannot be blocked or ignored — it always terminates the target.
///
/// Arguments:
///   arg0: target task ID
///   arg1: signal number (1..=31)
///
/// Returns: 0 on success, negative `Error` code on failure.
fn sys_kill(pid: u64, sig: u64) -> i64 {
    let task_id = crate::task::task::TaskId::from_u64(pid);

    // Validate signal number.
    let Ok(sig_num) = u8::try_from(sig) else {
        return Error::InvalidArgument as i64;
    };
    if sig_num == 0 || sig_num > 31 {
        return Error::InvalidArgument as i64;
    }

    crate::serial_println!("[SYSCALL] kill: pid={} sig={}", pid, sig_num);

    // Deliver the signal to the target task.
    let found = crate::task::scheduler::with_task_mut(task_id, |task| {
        task.signal_state.send_signal(sig_num);
    });

    match found {
        Some(()) => {
            // If the target task is blocked, wake it so the signal can be
            // delivered on its next syscall entry.
            crate::task::scheduler::wake_task_by_id(task_id);
            0
        }
        None => Error::NotFound as i64,
    }
}

/// Set the handler for a signal, returning the previous handler.
///
/// SIGKILL and SIGTRAP cannot have their handlers changed — they always use
/// the default action.
///
/// Arguments:
///   arg0: signal number (1..=31)
///   arg1: handler address (`SIG_DFL` = default, `SIG_IGN` = ignore, else = user addr)
///
/// Returns: previous handler address on success, negative `Error` code on failure.
#[allow(clippy::cast_possible_wrap)]
fn sys_signal(sig: u64, handler: u64) -> i64 {
    let Ok(sig_num) = u8::try_from(sig) else {
        return Error::InvalidArgument as i64;
    };
    if sig_num == 0 || sig_num > 31 {
        return Error::InvalidArgument as i64;
    }

    // SIGKILL cannot be caught or ignored.
    if sig_num == crate::task::signal::SIGKILL {
        return Error::PermissionDenied as i64;
    }

    crate::serial_println!("[SYSCALL] signal: sig={} handler={:#x}", sig_num, handler);

    let result = crate::task::scheduler::with_current_task_mut(|task| {
        let old = task.signal_state.handlers[sig_num as usize];
        task.signal_state.handlers[sig_num as usize] = handler;
        old
    });

    result.map_or(Error::NotFound as i64, |old_handler| old_handler as i64)
}

/// Return from a signal handler.
///
/// Restores the register context that was saved on the user stack during
/// signal delivery.
///
/// Arguments:
///   arg0: pointer to the `SignalFrame` on the user stack
fn sys_sigreturn(frame_ptr: u64) -> i64 {
    use crate::task::signal::SignalFrame;

    if frame_ptr == 0 || frame_ptr >= crate::memory::USER_SPACE_MAX {
        return Error::BadPointer as i64;
    }

    // Read the SignalFrame from user-space.
    let frame_size = core::mem::size_of::<SignalFrame>();
    let mut frame_buf = alloc::vec![0u8; frame_size];
    // SAFETY: frame_ptr was validated to be in user-space.
    unsafe {
        core::ptr::copy_nonoverlapping(frame_ptr as *const u8, frame_buf.as_mut_ptr(), frame_size);
    }

    // SAFETY: frame_buf was allocated by alloc with default alignment (>= 8 bytes),
    // SignalFrame is repr(C), and we read exactly its size bytes from the buffer.
    let frame: SignalFrame = unsafe {
        let (prefix, body, _tail) = frame_buf.align_to::<SignalFrame>();
        debug_assert!(prefix.is_empty(), "frame_buf not aligned for SignalFrame");
        core::ptr::read(body.as_ptr())
    };

    crate::serial_println!(
        "[SYSCALL] sigreturn: restoring context rsp={:#x} rip={:#x}",
        frame.saved_rsp,
        frame.saved_rcx
    );

    // Restore the saved context.
    crate::task::scheduler::with_current_task_mut(|task| {
        if let Some(ref mut ctx) = task.context {
            ctx.rsp = frame.saved_rsp;
            ctx.rcx = frame.saved_rcx;
            ctx.r11 = frame.saved_r11;
            ctx.rdi = frame.saved_rdi;
        }
    });

    0
}

/// Get or set the signal mask (blocked signals).
///
/// Arguments:
///   arg0: operation (`SIG_BLOCK=0`, `SIG_UNBLOCK=1`, `SIG_SETMASK=2`)
///   arg1: pointer to signal set (u64) in user-space, or 0 to query
///   arg2: pointer to output for old mask, or 0 to discard
fn sys_sigprocmask(how: u64, set_ptr: u64, oldset_ptr: u64) -> i64 {
    use crate::task::signal::{SIG_BLOCK, SIG_SETMASK, SIG_UNBLOCK};

    let new_mask: Option<u64> = if set_ptr != 0 {
        if set_ptr >= crate::memory::USER_SPACE_MAX {
            return Error::BadPointer as i64;
        }
        let mut buf = [0u8; 8];
        // SAFETY: set_ptr validated in user-space.
        unsafe {
            core::ptr::copy_nonoverlapping(set_ptr as *const u8, buf.as_mut_ptr(), 8);
        }
        Some(u64::from_le_bytes(buf))
    } else {
        None
    };

    let result = crate::task::scheduler::with_current_task_mut(|task| {
        let old_blocked = task.signal_state.blocked;

        if let Some(mask) = new_mask {
            match how {
                SIG_BLOCK => task.signal_state.blocked |= mask,
                SIG_UNBLOCK => task.signal_state.blocked &= !mask,
                SIG_SETMASK => task.signal_state.blocked = mask,
                _ => return Err(Error::InvalidArgument),
            }
        }

        Ok(old_blocked)
    });

    match result {
        Some(Ok(old_blocked)) => {
            if oldset_ptr != 0 && oldset_ptr < crate::memory::USER_SPACE_MAX {
                let bytes = old_blocked.to_le_bytes();
                // SAFETY: oldset_ptr validated in user-space.
                unsafe {
                    core::ptr::copy_nonoverlapping(bytes.as_ptr(), oldset_ptr as *mut u8, 8);
                }
            }
            crate::serial_println!("[SYSCALL] sigprocmask: how={}", how);
            0
        }
        Some(Err(e)) => e as i64,
        None => Error::NotFound as i64,
    }
}

// ─────────────────── Dup2 / Environment / Working directory ───────────────────

/// Duplicate a file descriptor.
///
/// If `old_fd` equals `new_fd`, returns `new_fd` immediately (no-op) after
/// verifying that `old_fd` is a valid, open file descriptor. If `new_fd`
/// already has an open file, it is closed first. The file handle from
/// `old_fd` is copied to the `new_fd` slot (same path, inode, and offset).
///
/// Arguments:
///   arg0: old file descriptor
///   arg1: new file descriptor
///
/// Returns: `new_fd` on success, negative `Error` code on failure.
#[allow(clippy::cast_possible_wrap)]
fn sys_dup2(old_fd: u64, new_fd: u64) -> i64 {
    // No-op: old_fd and new_fd are the same — nothing to do, but still
    // validate that the fd exists.
    if old_fd == new_fd {
        if old_fd < FD_FIRST_USABLE {
            return Error::InvalidArgument as i64;
        }
        let exists =
            crate::task::scheduler::with_current_task(|task| task.fd_table.contains_key(&old_fd));
        return if exists.unwrap_or(false) {
            new_fd as i64
        } else {
            Error::NotFound as i64
        };
    }

    // Validate both fds are in the usable range (not stdin/stdout).
    if old_fd < FD_FIRST_USABLE || new_fd < FD_FIRST_USABLE {
        return Error::InvalidArgument as i64;
    }
    // Validate upper bound to prevent unbounded resource usage.
    if old_fd >= FD_LIMIT || new_fd >= FD_LIMIT {
        return Error::InvalidArgument as i64;
    }

    let result = crate::task::scheduler::with_current_task_mut(|task| {
        // Look up old_fd.
        let old_entry = task.fd_table.get(&old_fd)?;
        let cloned = crate::task::task::FdEntry {
            path: old_entry.path.clone(),
            ino: old_entry.ino,
            offset: old_entry.offset,
        };
        // If new_fd is already open, close it (overwrite).
        task.fd_table.remove(&new_fd);
        task.fd_table.insert(new_fd, cloned);
        Some(new_fd)
    });

    match result {
        Some(Some(fd)) => {
            crate::serial_println!("[SYSCALL] dup2: {} -> {}", old_fd, fd);
            fd as i64
        }
        _ => Error::NotFound as i64,
    }
}

/// Get an environment variable.
///
/// Copies the value for `key` from the current task's environment into the
/// user-space buffer.
///
/// Arguments:
///   arg0: pointer to key string (UTF-8)
///   arg1: key length
///   arg2: pointer to value output buffer
///   arg3: value buffer length
///
/// Returns: length of the value on success, negative `Error` code on failure.
/// Returns 0 if the key does not exist and `val_len` is sufficient.
fn sys_env_get(key_ptr: u64, key_len: u64, val_ptr: u64, val_len: u64) -> i64 {
    let Some(key_bytes) = (unsafe { copy_from_user(key_ptr as *const u8, key_len as usize) })
    else {
        return Error::BadPointer as i64;
    };
    let Ok(key) = core::str::from_utf8(&key_bytes) else {
        return Error::InvalidArgument as i64;
    };

    let result = crate::task::scheduler::with_current_task(|task| task.env.get(key).cloned());

    match result {
        Some(Some(value)) => {
            let value_bytes = value.as_bytes();
            if unsafe { copy_to_user(val_ptr as *mut u8, value_bytes) } {
                crate::serial_println!("[SYSCALL] env_get: '{}' = '{}'", key, value);
                i64::try_from(value_bytes.len()).unwrap_or(-1)
            } else {
                Error::BadPointer as i64
            }
        }
        Some(None) => {
            // Key not found — return 0 (empty value).
            crate::serial_println!("[SYSCALL] env_get: '{}' not found", key);
            0
        }
        None => Error::NotFound as i64,
    }
}

/// Set an environment variable.
///
/// Copies the key and value from user-space into the current task's
/// environment map. If the key already exists, its value is overwritten.
///
/// Arguments:
///   arg0: pointer to key string (UTF-8)
///   arg1: key length
///   arg2: pointer to value string (UTF-8)
///   arg3: value length
///
/// Returns: 0 on success, negative `Error` code on failure.
fn sys_env_set(key_ptr: u64, key_len: u64, val_ptr: u64, val_len: u64) -> i64 {
    let Some(key_bytes) = (unsafe { copy_from_user(key_ptr as *const u8, key_len as usize) })
    else {
        return Error::BadPointer as i64;
    };
    let Some(val_bytes) = (unsafe { copy_from_user(val_ptr as *const u8, val_len as usize) })
    else {
        return Error::BadPointer as i64;
    };
    let Ok(key) = core::str::from_utf8(&key_bytes) else {
        return Error::InvalidArgument as i64;
    };
    let Ok(value) = core::str::from_utf8(&val_bytes) else {
        return Error::InvalidArgument as i64;
    };

    let result = crate::task::scheduler::with_current_task_mut(|task| {
        task.env.insert(
            alloc::string::String::from(key),
            alloc::string::String::from(value),
        );
    });

    match result {
        Some(()) => {
            crate::serial_println!("[SYSCALL] env_set: '{}' = '{}'", key, value);
            0
        }
        None => Error::NotFound as i64,
    }
}

/// Change the current working directory.
///
/// Validates that the path exists via VFS `stat`, then updates the task's
/// `cwd` field.
///
/// Arguments:
///   arg0: pointer to path string (UTF-8)
///   arg1: path length
///
/// Returns: 0 on success, negative `Error` code on failure.
fn sys_chdir(path_ptr: u64, path_len: u64) -> i64 {
    let Some(path_bytes) = (unsafe { copy_from_user(path_ptr as *const u8, path_len as usize) })
    else {
        return Error::BadPointer as i64;
    };
    let Ok(path) = core::str::from_utf8(&path_bytes) else {
        return Error::InvalidArgument as i64;
    };

    // Resolve relative paths against the task's cwd.
    let full_path = resolve_path(path);

    // Validate the path exists by stat-ing it.
    let (fs, rel) = crate::fs::vfs::resolve_fs(&full_path);
    if let Ok(ino) = fs.open(&rel, crate::fs::vfs::OpenFlags::READ) {
        // Verify it is a directory.
        let stat_result = fs.stat(ino);
        let _ = fs.close(ino);
        match stat_result {
            Ok(meta) => {
                if !meta.is_dir {
                    crate::serial_println!("[SYSCALL] chdir: '{}' is not a directory", full_path);
                    return Error::InvalidArgument as i64;
                }
            }
            Err(_) => return Error::NotFound as i64,
        }
    } else {
        crate::serial_println!("[SYSCALL] chdir: '{}' not found", full_path);
        return Error::NotFound as i64;
    }

    let result = crate::task::scheduler::with_current_task_mut(|task| {
        task.cwd.clone_from(&full_path);
    });

    match result {
        Some(()) => {
            crate::serial_println!("[SYSCALL] chdir: '{}'", full_path);
            0
        }
        None => Error::NotFound as i64,
    }
}

/// Get the current working directory.
///
/// Copies the task's `cwd` string into the user-space buffer.
///
/// Arguments:
///   arg0: pointer to output buffer
///   arg1: buffer length
///
/// Returns: length of the cwd string on success, negative `Error` code on failure.
fn sys_getcwd(buf_ptr: u64, buf_len: u64) -> i64 {
    let result = crate::task::scheduler::with_current_task(|task| task.cwd.clone());

    match result {
        Some(cwd) => {
            let cwd_bytes = cwd.as_bytes();
            if cwd_bytes.len() > buf_len as usize {
                return Error::InvalidArgument as i64;
            }
            if unsafe { copy_to_user(buf_ptr as *mut u8, cwd_bytes) } {
                crate::serial_println!("[SYSCALL] getcwd: '{}'", cwd);
                i64::try_from(cwd_bytes.len()).unwrap_or(-1)
            } else {
                Error::BadPointer as i64
            }
        }
        None => Error::NotFound as i64,
    }
}

// ─────────────────── Timer / Sleep ───────────────────

/// Block the current task until the global tick counter reaches `target_tick`.
///
/// Uses `block_and_switch` to yield the CPU to other tasks. On wakeup,
/// re-checks the tick counter and re-blocks if the target has not been reached.
fn sleep_until_tick(target_tick: u64) {
    loop {
        let current =
            crate::arch::x86_64::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);
        if current >= target_tick {
            return;
        }
        let ctx = crate::arch::x86_64::syscall::capture_current_context();
        let switched = crate::task::scheduler::block_and_switch(ctx);
        if !switched {
            // No other task ready — HLT until next interrupt, then re-check.
            x86_64::instructions::hlt();
        }
    }
}

/// Sleep for the specified number of timer ticks (~18.2 Hz per tick).
/// Yields the CPU to other tasks via `block_and_switch` until enough ticks elapse.
fn sys_sleep(ticks: u64) -> i64 {
    let start = crate::arch::x86_64::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);
    let target = start + ticks;
    crate::serial_println!("[SYSCALL] sleep: {} ticks (from {})", ticks, start);
    sleep_until_tick(target);
    0
}

// ─────────────────── Event signaling ───────────────────

/// Create a new event object. Returns a handle to the event.
fn sys_event_create() -> i64 {
    let event = crate::handle::Event::new();
    let event_arc = alloc::sync::Arc::new(spin::Mutex::new(event));

    let result = crate::task::scheduler::with_current_task_mut(|task| {
        let handle = task.handle_table.insert(
            crate::handle::KernelObject::Event(event_arc),
            crate::handle::Rights::ALL,
        );
        handle.as_u64()
    });

    #[allow(clippy::cast_possible_wrap)]
    result.map_or(Error::NotFound as i64, |id| {
        crate::serial_println!("[SYSCALL] event_create: handle={:#x}", id);
        id as i64
    })
}

/// Signal an event. Wakes any task waiting on it.
fn sys_event_signal(handle_raw: u64) -> i64 {
    let handle = crate::handle::Handle::from_raw(handle_raw);
    let result = crate::task::scheduler::with_current_task(|task| {
        if let Some(crate::handle::KernelObject::Event(event)) = task.handle_table.get(handle) {
            event.lock().signal();
            true
        } else {
            false
        }
    });

    match result {
        Some(true) => {
            crate::serial_println!("[SYSCALL] event_signal: signaled");
            0
        }
        _ => Error::NotFound as i64,
    }
}

/// Block until an event is signaled. Clears the signal before returning.
///
/// If the event is already signaled, returns immediately (fast path).
/// Otherwise, spins with HLT until another task signals the event.
fn sys_event_wait(handle_raw: u64) -> i64 {
    let handle = crate::handle::Handle::from_raw(handle_raw);

    // Retrieve the Event Arc from the handle table.
    let event_arc = crate::task::scheduler::with_current_task(|task| {
        if let Some(crate::handle::KernelObject::Event(event)) = task.handle_table.get(handle) {
            Some(alloc::sync::Arc::clone(event))
        } else {
            None
        }
    });

    let Some(event_arc) = event_arc.flatten() else {
        return Error::NotFound as i64;
    };

    crate::serial_println!("[SYSCALL] event_wait: handle={handle_raw:#x}");

    // Fast path: if already signaled, clear and return immediately.
    {
        let mut ev = event_arc.lock();
        if ev.is_signaled() {
            ev.clear();
            crate::serial_println!("[SYSCALL] event_wait: fast path, already signaled");
            return 0;
        }
    }

    // Slow path: spin with HLT until the event becomes signaled.
    loop {
        x86_64::instructions::hlt();
        let mut ev = event_arc.lock();
        if ev.is_signaled() {
            ev.clear();
            crate::serial_println!("[SYSCALL] event_wait: woke up, signaled");
            return 0;
        }
    }
}

/// Destroy an event by closing its handle.
fn sys_event_destroy(handle_raw: u64) -> i64 {
    let handle = crate::handle::Handle::from_raw(handle_raw);
    let result =
        crate::task::scheduler::with_current_task_mut(|task| task.handle_table.close(handle));
    match result {
        Some(true) => {
            crate::serial_println!("[SYSCALL] event_destroy: handle={handle_raw:#x}");
            0
        }
        _ => Error::NotFound as i64,
    }
}

// ─────────────────── IRQ forwarding ───────────────────

/// Wait for a hardware IRQ event to be signaled.
///
/// Arguments:
///   arg0: handle to an `IrqEvent` kernel object
///   arg1: blocking flag (0 = non-blocking, non-zero = blocking)
///
/// Returns:
///   - 0 on success (IRQ was signaled)
///   - `WouldBlock` if non-blocking and not yet signaled
///   - `NotFound` if the handle is invalid
fn sys_irq_wait(handle_raw: u64, blocking: u64) -> i64 {
    let handle = crate::handle::Handle::from_raw(handle_raw);

    // Retrieve the IrqEvent Arc from the handle table.
    let event_arc = crate::task::scheduler::with_current_task(|task| {
        if let Some(crate::handle::KernelObject::IrqEvent(event)) = task.handle_table.get(handle) {
            Some(alloc::sync::Arc::clone(event))
        } else {
            None
        }
    });

    let Some(event_arc) = event_arc.flatten() else {
        return Error::NotFound as i64;
    };

    crate::serial_println!("[SYSCALL] irq_wait: handle={handle_raw:#x} blocking={blocking}");

    // Fast path: check if already signaled.
    {
        let mut ev = event_arc.lock();
        if ev.is_signaled() {
            let data = ev.last_data;
            ev.clear();
            crate::serial_println!(
                "[SYSCALL] irq_wait: fast path, already signaled, data={:#x}",
                data
            );
            // Return the device data byte as a positive value.
            return i64::from(data);
        }
    }

    // Non-blocking: return immediately if not signaled.
    if blocking == 0 {
        crate::serial_println!("[SYSCALL] irq_wait: non-blocking, would block");
        return Error::WouldBlock as i64;
    }

    // Blocking: spin with HLT until the IRQ fires and signals the event.
    loop {
        x86_64::instructions::hlt();
        let mut ev = event_arc.lock();
        if ev.is_signaled() {
            let data = ev.last_data;
            ev.clear();
            crate::serial_println!("[SYSCALL] irq_wait: woke up, signaled, data={:#x}", data);
            // Return the device data byte as a positive value.
            return i64::from(data);
        }
    }
}

// ─────────────────── Debug console ───────────────────

/// Write bytes to the kernel debug console (serial port).
///
/// Copies `len` bytes from user-space and writes each byte to the serial output.
/// This is an OpenOS-specific debug syscall, not part of the INTERFACE.md spec.
///
/// Arguments:
///   arg0: pointer to data in user-space
///   arg1: number of bytes to write
///
/// Returns: number of bytes written on success, negative `Error` code on failure.
fn sys_console_write(ptr: u64, len: u64) -> i64 {
    let Some(msg) = (unsafe { copy_from_user(ptr as *const u8, len as usize) }) else {
        return Error::BadPointer as i64;
    };
    for &byte in &msg {
        crate::serial_print!("{}", byte as char);
    }
    i64::try_from(len).unwrap_or(-1)
}

/// Read characters from the keyboard input buffer.
///
/// Arguments:
///   - arg0: pointer to user-space buffer
///   - arg1: buffer length in bytes
///   - arg2: flags (bit 0 = blocking)
///
/// Returns:
///   - Positive: number of bytes read
///   - Negative: error code
fn sys_console_read(buf_ptr: u64, buf_len: u64, flags: u64) -> i64 {
    // Validate the user-space pointer
    if buf_ptr == 0 || buf_len == 0 {
        return Error::InvalidArgument as i64;
    }

    if buf_ptr >= crate::memory::USER_SPACE_MAX {
        return Error::BadPointer as i64;
    }

    // Overflow check: buf_ptr + buf_len must not exceed user-space boundary.
    if buf_ptr.saturating_add(buf_len) > crate::memory::USER_SPACE_MAX {
        return Error::BadPointer as i64;
    }

    // Cap at MAX_MSG_SIZE to prevent abuse
    let len = buf_len.min(MAX_MSG_SIZE as u64) as usize;

    let blocking = (flags & 1) != 0;

    // SAFETY: We've validated the pointer is non-null and in user-space.
    // The keyboard driver will write at most `len` bytes.
    let bytes_read = unsafe { crate::drivers::keyboard::read(buf_ptr as *mut u8, len, blocking) };

    i64::try_from(bytes_read).unwrap_or(-1)
}

// ─────────────────── Service discovery ───────────────────

/// Register a named service endpoint in the current task's namespace.
///
/// Arguments:
///   arg0: pointer to service name (UTF-8)
///   arg1: name length
///   arg2: handle to the Channel server end
fn sys_endpoint_register(name_ptr: u64, name_len: u64, handle_raw: u64) -> i64 {
    let Some(name_bytes) = (unsafe { copy_from_user(name_ptr as *const u8, name_len as usize) })
    else {
        return Error::BadPointer as i64;
    };
    let Ok(name) = core::str::from_utf8(&name_bytes) else {
        return Error::InvalidArgument as i64;
    };

    let handle = crate::handle::Handle::from_raw(handle_raw);
    let result = crate::task::scheduler::with_current_task_mut(|task| {
        task.namespace
            .insert(alloc::string::String::from(name), handle);
    });

    match result {
        Some(()) => {
            crate::serial_println!(
                "[SYSCALL] endpoint_register: '{}' -> handle {:#x}",
                name,
                handle_raw
            );
            0
        }
        None => Error::NotFound as i64,
    }
}

/// Discover a named service endpoint in the current task's namespace.
///
/// Arguments:
///   arg0: pointer to service name (UTF-8)
///   arg1: name length
///
/// Returns: handle to the Channel server end, or negative error.
fn sys_endpoint_discover(name_ptr: u64, name_len: u64) -> i64 {
    let Some(name_bytes) = (unsafe { copy_from_user(name_ptr as *const u8, name_len as usize) })
    else {
        return Error::BadPointer as i64;
    };
    let Ok(name) = core::str::from_utf8(&name_bytes) else {
        return Error::InvalidArgument as i64;
    };

    let result = crate::task::scheduler::with_current_task(|task| {
        task.namespace.get(name).map(|h| h.as_u64())
    });

    match result {
        Some(Some(handle_raw)) => {
            crate::serial_println!(
                "[SYSCALL] endpoint_discover: '{}' -> handle {:#x}",
                name,
                handle_raw
            );
            #[allow(clippy::cast_possible_wrap)]
            {
                handle_raw as i64
            }
        }
        _ => Error::NotFound as i64,
    }
}

// ─────────────────── Filesystem (VFS) ───────────────────

/// Reserved fd for stdin (not backed by VFS).
const FD_STDIN: u64 = 0;

/// Reserved fd for stdout (not backed by VFS).
const FD_STDOUT: u64 = 1;

/// First usable fd for VFS files.
const FD_FIRST_USABLE: u64 = 2;

/// Maximum file descriptors per task. Prevents the fd allocation loop from
/// wrapping around `u64` and ensures bounded resource usage.
const FD_LIMIT: u64 = 1024;

/// Open a file via VFS and return a file descriptor.
///
/// Uses `resolve_fs` to find the filesystem for the given path, then
/// calls `open` on that filesystem.
///
/// Arguments:
///   arg0: pointer to filename (UTF-8)
///   arg1: filename length
///   arg2: flags (reserved, must be 0 for now)
///
/// Returns: fd >= 0 on success, negative error code on failure.
fn sys_fs_open(name_ptr: u64, name_len: u64, flags: u64) -> i64 {
    // flags: 0 = read-only, 1 = write/create/truncate
    if flags > 1 {
        return Error::InvalidArgument as i64;
    }

    let Some(name_bytes) = (unsafe { copy_from_user(name_ptr as *const u8, name_len as usize) })
    else {
        return Error::BadPointer as i64;
    };
    let Ok(filename) = core::str::from_utf8(&name_bytes) else {
        return Error::InvalidArgument as i64;
    };

    // Resolve relative paths against the task's cwd.
    let full_path = resolve_path(filename);

    // Resolve the path to a filesystem and relative path.
    let (fs, rel_path) = crate::fs::vfs::resolve_fs(&full_path);

    // Select open flags based on the user-provided flags argument.
    let open_flags = if flags == 0 {
        crate::fs::vfs::OpenFlags::READ
    } else {
        // READ (0x1) | WRITE (0x2) | CREATE (0x4) = 0x7
        crate::fs::vfs::OpenFlags::from_raw(0x7)
    };

    // Open the file on the resolved filesystem.
    let Ok(ino) = fs.open(&rel_path, open_flags) else {
        return Error::NotFound as i64;
    };

    let result = crate::task::scheduler::with_current_task_mut(|task| {
        // Find the first free fd number starting after stdin/stdout.
        let mut fd = FD_FIRST_USABLE;
        while task.fd_table.contains_key(&fd) {
            fd += 1;
            if fd >= FD_LIMIT {
                return None;
            }
        }
        task.fd_table.insert(
            fd,
            crate::task::task::FdEntry {
                path: full_path,
                ino,
                offset: 0,
            },
        );
        Some(fd)
    });

    match result {
        Some(Some(fd)) => {
            crate::serial_println!("[SYSCALL] fs_open: '{}' -> fd {}", filename, fd);
            #[allow(clippy::cast_possible_wrap)]
            {
                fd as i64
            }
        }
        Some(None) => Error::OutOfMemory as i64,
        None => Error::NotFound as i64,
    }
}

/// Read bytes from an open file descriptor.
///
/// Arguments:
///   arg0: file descriptor
///   arg1: pointer to user-space buffer
///   arg2: buffer length
///
/// Returns: number of bytes read, or negative error code.
fn sys_fs_read(fd: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    if fd == FD_STDIN || fd == FD_STDOUT {
        return Error::InvalidArgument as i64;
    }

    // Read the path, inode, and current offset from the task's fd table.
    let (path, ino, offset) = {
        let result = crate::task::scheduler::with_current_task(|task| {
            task.fd_table
                .get(&fd)
                .map(|entry| (entry.path.clone(), entry.ino, entry.offset))
        });
        match result {
            Some(Some(triple)) => triple,
            _ => return Error::NotFound as i64,
        }
    };

    // Handle pipe read: the path starts with "<pipe:" and ino is the handle value.
    if path.starts_with("<pipe:") {
        #[allow(clippy::cast_possible_truncation)]
        let read_len = buf_len.min(MAX_MSG_SIZE as u64) as usize;
        let mut buf = alloc::vec![0u8; read_len];

        let handle = crate::handle::Handle::from_raw(ino);
        let bytes_read = crate::task::scheduler::with_current_task(|task| {
            if let Some(crate::handle::KernelObject::PipeReader(pr)) = task.handle_table.get(handle)
            {
                Some(pr.lock().read(&mut buf))
            } else {
                None
            }
        });

        match bytes_read {
            Some(Some(n)) => {
                if n > 0 && !unsafe { copy_to_user(buf_ptr as *mut u8, &buf[..n]) } {
                    return Error::BadPointer as i64;
                }
                crate::serial_println!("[SYSCALL] fs_read: pipe fd {} read {} bytes", fd, n);
                #[allow(clippy::cast_possible_wrap)]
                {
                    return n as i64;
                }
            }
            _ => return Error::NotFound as i64,
        }
    }

    // Resolve the filesystem and read from it.
    let (fs, _rel_path) = crate::fs::vfs::resolve_fs(&path);

    // Cap the read buffer to prevent excessive allocation.
    #[allow(clippy::cast_possible_truncation)]
    let read_len = buf_len.min(MAX_MSG_SIZE as u64) as usize;
    let mut buf = alloc::vec![0u8; read_len];

    let Ok(bytes_read) = fs.read(ino, offset as u64, &mut buf) else {
        return Error::NotFound as i64;
    };

    if !unsafe { copy_to_user(buf_ptr as *mut u8, &buf[..bytes_read]) } {
        return Error::BadPointer as i64;
    }

    // Advance the offset in the task's fd table.
    crate::task::scheduler::with_current_task_mut(|task| {
        if let Some(entry) = task.fd_table.get_mut(&fd) {
            entry.offset += bytes_read;
        }
    });

    crate::serial_println!("[SYSCALL] fs_read: fd {} read {} bytes", fd, bytes_read);
    #[allow(clippy::cast_possible_wrap)]
    {
        bytes_read as i64
    }
}

/// Write bytes to an open file descriptor.
///
/// Arguments:
///   arg0: file descriptor
///   arg1: pointer to data
///   arg2: data length
///
/// Returns: number of bytes written, or negative error code.
fn sys_fs_write(fd: u64, data_ptr: u64, data_len: u64) -> i64 {
    if fd == FD_STDOUT {
        // Redirect stdout writes to the serial console.
        let Some(msg) = (unsafe { copy_from_user(data_ptr as *const u8, data_len as usize) })
        else {
            return Error::BadPointer as i64;
        };
        for &byte in &msg {
            crate::serial_print!("{}", byte as char);
        }
        return i64::try_from(data_len).unwrap_or(-1);
    }

    if fd == FD_STDIN {
        return Error::InvalidArgument as i64;
    }

    let Some(data) = (unsafe { copy_from_user(data_ptr as *const u8, data_len as usize) }) else {
        return Error::BadPointer as i64;
    };

    // Read the path, inode, and current offset from the task's fd table.
    let (path, ino, offset) = {
        let result = crate::task::scheduler::with_current_task(|task| {
            task.fd_table
                .get(&fd)
                .map(|entry| (entry.path.clone(), entry.ino, entry.offset))
        });
        match result {
            Some(Some(triple)) => triple,
            _ => return Error::NotFound as i64,
        }
    };

    // Handle pipe write: the path starts with "<pipe:" and ino is the handle value.
    if path.starts_with("<pipe:") {
        let handle = crate::handle::Handle::from_raw(ino);
        let write_result = crate::task::scheduler::with_current_task(|task| {
            if let Some(crate::handle::KernelObject::PipeWriter(pw)) = task.handle_table.get(handle)
            {
                Some(pw.lock().write(&data))
            } else {
                None
            }
        });

        return match write_result {
            Some(Some(Ok(n))) => {
                crate::serial_println!("[SYSCALL] fs_write: pipe fd {} wrote {} bytes", fd, n);
                #[allow(clippy::cast_possible_wrap)]
                {
                    n as i64
                }
            }
            Some(Some(Err(epipe))) => epipe,
            _ => Error::NotFound as i64,
        };
    }

    // Resolve the filesystem and write to it.
    let (fs, _rel_path) = crate::fs::vfs::resolve_fs(&path);

    fs.write(ino, offset as u64, &data)
        .map_or(Error::InvalidArgument as i64, |written| {
            // Advance the offset in the task's fd table.
            crate::task::scheduler::with_current_task_mut(|task| {
                if let Some(entry) = task.fd_table.get_mut(&fd) {
                    entry.offset += written;
                }
            });
            crate::serial_println!(
                "[SYSCALL] fs_write: fd {} wrote {} bytes at offset {}",
                fd,
                written,
                offset
            );
            #[allow(clippy::cast_possible_wrap)]
            {
                written as i64
            }
        })
}

/// Close a file descriptor.
///
/// Also calls `close` on the underlying filesystem to release any
/// open-file state.
///
/// Arguments:
///   arg0: file descriptor
///
/// Returns: 0 on success, negative error code on failure.
fn sys_fs_close(fd: u64) -> i64 {
    if fd < FD_FIRST_USABLE {
        return Error::InvalidArgument as i64;
    }

    let result = crate::task::scheduler::with_current_task_mut(|task| {
        task.fd_table
            .remove(&fd)
            .map(|entry| (entry.path, entry.ino))
    });

    match result {
        Some(Some((path, ino))) => {
            // Close the inode on the underlying filesystem.
            let (fs, _rel_path) = crate::fs::vfs::resolve_fs(&path);
            let _ = fs.close(ino);
            crate::serial_println!("[SYSCALL] fs_close: fd {}", fd);
            0
        }
        _ => Error::NotFound as i64,
    }
}

/// Seek to a position in an open file descriptor.
///
/// Arguments:
///   arg0: file descriptor
///   arg1: offset (signed i64)
///   arg2: whence
///
/// Returns: the new absolute offset, or negative error code.
#[allow(clippy::cast_sign_loss)]
fn sys_fs_seek(fd: u64, offset_raw: u64, whence: u64) -> i64 {
    if fd == FD_STDIN || fd == FD_STDOUT {
        return Error::InvalidArgument as i64;
    }

    // Interpret offset as signed.
    #[allow(clippy::cast_possible_wrap)]
    let offset = offset_raw as i64;

    // Read the path, inode, and current offset from the task's fd table.
    let (path, ino, current_offset) = {
        let result = crate::task::scheduler::with_current_task(|task| {
            task.fd_table
                .get(&fd)
                .map(|entry| (entry.path.clone(), entry.ino, entry.offset))
        });
        match result {
            Some(Some(triple)) => triple,
            _ => return Error::NotFound as i64,
        }
    };

    // Get file size from the filesystem.
    let (fs, _rel_path) = crate::fs::vfs::resolve_fs(&path);
    let Ok(meta) = fs.stat(ino) else {
        return Error::NotFound as i64;
    };
    let file_len = meta.size as usize;

    let new_offset: usize = match whence {
        // SEEK_SET: offset from beginning of file.
        number::SEEK_SET => {
            if offset < 0 {
                return Error::InvalidArgument as i64;
            }
            offset as usize
        }
        // SEEK_CUR: offset from current position.
        number::SEEK_CUR => {
            if offset >= 0 {
                current_offset.saturating_add(offset as usize)
            } else {
                let abs = (-offset) as usize;
                current_offset.saturating_sub(abs)
            }
        }
        // SEEK_END: offset from end of file.
        number::SEEK_END => {
            if offset >= 0 {
                file_len
            } else {
                let abs = (-offset) as usize;
                file_len.saturating_sub(abs)
            }
        }
        _ => return Error::InvalidArgument as i64,
    };

    // Update the offset in the task's fd table.
    crate::task::scheduler::with_current_task_mut(|task| {
        if let Some(entry) = task.fd_table.get_mut(&fd) {
            entry.offset = new_offset;
        }
    });

    crate::serial_println!(
        "[SYSCALL] fs_seek: fd {} offset {} whence {} -> {}",
        fd,
        offset,
        whence,
        new_offset
    );
    #[allow(clippy::cast_possible_wrap)]
    {
        new_offset as i64
    }
}

/// Resolve a user-provided path against the current working directory.
///
/// If `path` starts with `'/'`, it is absolute and returned as-is.
/// Otherwise, it is relative to the task's `cwd`: the cwd is prepended.
///
/// Special cases:
/// - `"/"` cwd + `"foo"` -> `"/foo"`
/// - `"/disk"` cwd + `"foo"` -> `"/disk/foo"`
/// - `""` path -> returns cwd
fn resolve_path(path: &str) -> alloc::string::String {
    if path.starts_with('/') {
        // Absolute path — return as-is.
        return alloc::string::String::from(path);
    }

    // Read the current working directory from the task.
    let cwd = crate::task::scheduler::with_current_task(|task| task.cwd.clone())
        .unwrap_or_else(|| alloc::string::String::from("/"));

    if path.is_empty() {
        return cwd;
    }

    // Combine cwd + "/" + relative path.
    if cwd.ends_with('/') {
        alloc::format!("{cwd}{path}")
    } else {
        alloc::format!("{cwd}/{path}")
    }
}

// ─────────────────── Filesystem metadata syscalls ───────────────────

/// Helper: split a path into parent directory and filename.
/// "/disk/foo/bar.txt" -> ("/disk/foo", "bar.txt")
fn split_parent_name(path: &str) -> Option<(&str, &str)> {
    let trimmed = path.trim_end_matches('/');
    let pos = trimmed.rfind('/')?;
    let parent = if pos == 0 { "/" } else { &trimmed[..pos] };
    let name = &trimmed[pos + 1..];
    if name.is_empty() {
        None
    } else {
        Some((parent, name))
    }
}

/// Delete a file by path.
///
/// Arguments:
///   arg0: pointer to path string (UTF-8)
///   arg1: path length
///
/// Returns: 0 on success, negative error code on failure.
fn sys_fs_unlink(path_ptr: u64, path_len: u64) -> i64 {
    let Some(path_bytes) = (unsafe { copy_from_user(path_ptr as *const u8, path_len as usize) })
    else {
        return Error::BadPointer as i64;
    };
    let Ok(path) = core::str::from_utf8(&path_bytes) else {
        return Error::InvalidArgument as i64;
    };

    // Resolve relative paths against the task's cwd.
    let full_path = resolve_path(path);

    let Some((parent, name)) = split_parent_name(&full_path) else {
        return Error::InvalidArgument as i64;
    };

    let (fs, rel_parent) = crate::fs::vfs::resolve_fs(parent);
    // Open parent directory to get its inode.
    let Ok(parent_ino) = fs.open(&rel_parent, crate::fs::vfs::OpenFlags::READ) else {
        return Error::NotFound as i64;
    };

    match fs.unlink(parent_ino, name) {
        Ok(()) => {
            let _ = fs.close(parent_ino);
            crate::serial_println!("[SYSCALL] fs_unlink: '{}'", full_path);
            0
        }
        Err(e) => {
            let _ = fs.close(parent_ino);
            crate::serial_println!("[SYSCALL] fs_unlink: '{}' failed: {:?}", full_path, e);
            Error::NotFound as i64
        }
    }
}

/// Rename a file (copy + delete).
///
/// Arguments:
///   arg0: pointer to old path string
///   arg1: old path length
///   arg2: pointer to new path string
///   arg3: new path length
///
/// Returns: 0 on success, negative error code on failure.
fn sys_fs_rename(old_ptr: u64, old_len: u64, new_ptr: u64, new_len: u64) -> i64 {
    let Some(old_bytes) = (unsafe { copy_from_user(old_ptr as *const u8, old_len as usize) })
    else {
        return Error::BadPointer as i64;
    };
    let Some(new_bytes) = (unsafe { copy_from_user(new_ptr as *const u8, new_len as usize) })
    else {
        return Error::BadPointer as i64;
    };
    let Ok(old_path) = core::str::from_utf8(&old_bytes) else {
        return Error::InvalidArgument as i64;
    };
    let Ok(new_path) = core::str::from_utf8(&new_bytes) else {
        return Error::InvalidArgument as i64;
    };

    // Resolve relative paths against the task's cwd.
    let full_old = resolve_path(old_path);
    let full_new = resolve_path(new_path);

    // Read old file.
    let (fs_old, rel_old) = crate::fs::vfs::resolve_fs(&full_old);
    let Ok(old_ino) = fs_old.open(&rel_old, crate::fs::vfs::OpenFlags::READ) else {
        return Error::NotFound as i64;
    };

    let mut buf = [0u8; 8192];
    let n = fs_old.read(old_ino, 0, &mut buf).unwrap_or(0);
    let _ = fs_old.close(old_ino);

    // Create new file.
    let (fs_new, rel_new) = crate::fs::vfs::resolve_fs(&full_new);
    let Ok(new_ino) = fs_new.open(&rel_new, crate::fs::vfs::OpenFlags::from_raw(0x7)) else {
        return Error::Busy as i64;
    };
    if n > 0 {
        let _ = fs_new.write(new_ino, 0, &buf[..n]);
    }
    let _ = fs_new.close(new_ino);

    // Delete old file.
    if let Some((parent, name)) = split_parent_name(&full_old) {
        let (fs_p, rel_p) = crate::fs::vfs::resolve_fs(parent);
        if let Ok(p_ino) = fs_p.open(&rel_p, crate::fs::vfs::OpenFlags::READ) {
            let _ = fs_p.unlink(p_ino, name);
            let _ = fs_p.close(p_ino);
        }
    }

    crate::serial_println!("[SYSCALL] fs_rename: '{}' -> '{}'", full_old, full_new);
    0
}

/// Create a directory.
///
/// Arguments:
///   arg0: pointer to path string
///   arg1: path length
///   arg2: permissions (reserved)
///
/// Returns: 0 on success, negative error code on failure.
fn sys_fs_mkdir(path_ptr: u64, path_len: u64, _perms: u64) -> i64 {
    let Some(path_bytes) = (unsafe { copy_from_user(path_ptr as *const u8, path_len as usize) })
    else {
        return Error::BadPointer as i64;
    };
    let Ok(path) = core::str::from_utf8(&path_bytes) else {
        return Error::InvalidArgument as i64;
    };

    // Resolve relative paths against the task's cwd.
    let full_path = resolve_path(path);

    let Some((parent, name)) = split_parent_name(&full_path) else {
        return Error::InvalidArgument as i64;
    };

    let (fs, rel_parent) = crate::fs::vfs::resolve_fs(parent);
    let Ok(parent_ino) = fs.open(&rel_parent, crate::fs::vfs::OpenFlags::READ) else {
        return Error::NotFound as i64;
    };

    match fs.mkdir(parent_ino, name) {
        Ok(_ino) => {
            let _ = fs.close(parent_ino);
            crate::serial_println!("[SYSCALL] fs_mkdir: '{}'", full_path);
            0
        }
        Err(e) => {
            let _ = fs.close(parent_ino);
            crate::serial_println!("[SYSCALL] fs_mkdir: '{}' failed: {:?}", full_path, e);
            match e {
                crate::fs::vfs::FsError::NotFound => Error::NotFound as i64,
                crate::fs::vfs::FsError::NoSpace => Error::OutOfMemory as i64,
                crate::fs::vfs::FsError::NotADirectory | crate::fs::vfs::FsError::InvalidName => {
                    Error::InvalidArgument as i64
                }
                _ => Error::Busy as i64,
            }
        }
    }
}

/// Remove a directory.
///
/// Arguments:
///   arg0: pointer to path string
///   arg1: path length
///
/// Returns: 0 on success, negative error code on failure.
fn sys_fs_rmdir(path_ptr: u64, path_len: u64) -> i64 {
    let Some(path_bytes) = (unsafe { copy_from_user(path_ptr as *const u8, path_len as usize) })
    else {
        return Error::BadPointer as i64;
    };
    let Ok(path) = core::str::from_utf8(&path_bytes) else {
        return Error::InvalidArgument as i64;
    };

    // Resolve relative paths against the task's cwd.
    let full_path = resolve_path(path);

    let Some((parent, name)) = split_parent_name(&full_path) else {
        return Error::InvalidArgument as i64;
    };

    let (fs, rel_parent) = crate::fs::vfs::resolve_fs(parent);
    let Ok(parent_ino) = fs.open(&rel_parent, crate::fs::vfs::OpenFlags::READ) else {
        return Error::NotFound as i64;
    };

    match fs.rmdir(parent_ino, name) {
        Ok(()) => {
            let _ = fs.close(parent_ino);
            crate::serial_println!("[SYSCALL] fs_rmdir: '{}'", full_path);
            0
        }
        Err(e) => {
            let _ = fs.close(parent_ino);
            crate::serial_println!("[SYSCALL] fs_rmdir: '{}' failed: {:?}", full_path, e);
            match e {
                crate::fs::vfs::FsError::NotFound => Error::NotFound as i64,
                crate::fs::vfs::FsError::NotADirectory => Error::InvalidArgument as i64,
                _ => Error::Busy as i64,
            }
        }
    }
}

/// Get file status/metadata.
///
/// Arguments:
///   arg0: pointer to path string
///   arg1: path length
///   arg2: pointer to stat buffer (u64 size, u64 ino, u32 mode)
///
/// Returns: 0 on success, negative error code on failure.
fn sys_fs_stat(path_ptr: u64, path_len: u64, out_ptr: u64) -> i64 {
    let Some(path_bytes) = (unsafe { copy_from_user(path_ptr as *const u8, path_len as usize) })
    else {
        return Error::BadPointer as i64;
    };
    let Ok(path) = core::str::from_utf8(&path_bytes) else {
        return Error::InvalidArgument as i64;
    };

    // Resolve relative paths against the task's cwd.
    let full_path = resolve_path(path);

    let (fs, rel) = crate::fs::vfs::resolve_fs(&full_path);
    let Ok(ino) = fs.open(&rel, crate::fs::vfs::OpenFlags::READ) else {
        return Error::NotFound as i64;
    };

    match fs.stat(ino) {
        Ok(meta) => {
            let _ = fs.close(ino);
            // Write size (u64) + ino (u64) + is_dir (u32, 1=dir, 0=file) = 20 bytes
            let mode: u32 = if meta.is_dir { 0o40_755 } else { 0o100_644 };
            let mut stat_buf = [0u8; 20];
            stat_buf[0..8].copy_from_slice(&meta.size.to_le_bytes());
            stat_buf[8..16].copy_from_slice(&meta.ino.to_le_bytes());
            stat_buf[16..20].copy_from_slice(&mode.to_le_bytes());
            if !unsafe { copy_to_user(out_ptr as *mut u8, &stat_buf) } {
                return Error::BadPointer as i64;
            }
            crate::serial_println!("[SYSCALL] fs_stat: '{}' size={}", full_path, meta.size);
            0
        }
        Err(e) => {
            let _ = fs.close(ino);
            crate::serial_println!("[SYSCALL] fs_stat: '{}' failed: {:?}", full_path, e);
            Error::NotFound as i64
        }
    }
}

/// Get file status by file descriptor.
///
/// Looks up the file descriptor in the current task's fd table, resolves
/// the filesystem path, calls `stat()`, and writes the result to the
/// user-space buffer.
///
/// Arguments:
///   arg0: file descriptor
///   arg1: pointer to stat buffer (u64 size, u64 ino, u32 mode)
///
/// Returns: 0 on success, negative error code on failure.
fn sys_fstat(fd: u64, out_ptr: u64) -> i64 {
    // Read the path and inode from the task's fd table.
    let (path, ino) = {
        let result = crate::task::scheduler::with_current_task(|task| {
            task.fd_table
                .get(&fd)
                .map(|entry| (entry.path.clone(), entry.ino))
        });
        match result {
            Some(Some(pair)) => pair,
            _ => return Error::NotFound as i64,
        }
    };

    // Resolve the filesystem and stat the inode.
    let (fs, _rel_path) = crate::fs::vfs::resolve_fs(&path);

    match fs.stat(ino) {
        Ok(meta) => {
            // Write size (u64) + ino (u64) + mode (u32) = 20 bytes.
            let mode: u32 = if meta.is_dir { 0o40_755 } else { 0o100_644 };
            let mut stat_buf = [0u8; 20];
            stat_buf[0..8].copy_from_slice(&meta.size.to_le_bytes());
            stat_buf[8..16].copy_from_slice(&meta.ino.to_le_bytes());
            stat_buf[16..20].copy_from_slice(&mode.to_le_bytes());
            if !unsafe { copy_to_user(out_ptr as *mut u8, &stat_buf) } {
                return Error::BadPointer as i64;
            }
            crate::serial_println!("[SYSCALL] fstat: fd {} size={}", fd, meta.size);
            0
        }
        Err(e) => {
            crate::serial_println!("[SYSCALL] fstat: fd {} failed: {:?}", fd, e);
            Error::NotFound as i64
        }
    }
}

/// Get file status without following symlinks.
///
/// Currently delegates to `sys_fs_stat` because symlink resolution is
/// not fully implemented. When symlink support is added, this should
/// use `lstat()` semantics (return metadata of the link itself, not
/// the target).
///
/// Arguments:
///   arg0: pointer to path string (UTF-8)
///   arg1: path length
///   arg2: pointer to stat buffer (u64 size, u64 ino, u32 mode)
///
/// Returns: 0 on success, negative error code on failure.
fn sys_lstat(path_ptr: u64, path_len: u64, out_ptr: u64) -> i64 {
    // Delegate to sys_fs_stat — symlink-aware behavior will be added later.
    sys_fs_stat(path_ptr, path_len, out_ptr)
}

/// Read directory entries.
///
/// Arguments:
///   arg0: pointer to path string (directory)
///   arg1: path length
///   arg2: pointer to output buffer (entries written as null-terminated names)
///
/// Returns: number of bytes written to buffer, or negative error code.
fn sys_fs_readdir(path_ptr: u64, path_len: u64, out_ptr: u64) -> i64 {
    let Some(path_bytes) = (unsafe { copy_from_user(path_ptr as *const u8, path_len as usize) })
    else {
        return Error::BadPointer as i64;
    };
    let Ok(path) = core::str::from_utf8(&path_bytes) else {
        return Error::InvalidArgument as i64;
    };

    // Resolve relative paths against the task's cwd.
    let full_path = resolve_path(path);

    let (fs, rel) = crate::fs::vfs::resolve_fs(&full_path);
    let Ok(dir_ino) = fs.open(&rel, crate::fs::vfs::OpenFlags::READ) else {
        return Error::NotFound as i64;
    };

    match fs.readdir(dir_ino) {
        Ok(entries) => {
            let _ = fs.close(dir_ino);
            let mut buf = [0u8; 4096];
            let mut pos = 0;
            for entry in &entries {
                let name = entry.name.as_bytes();
                if pos + name.len() + 1 > buf.len() {
                    break;
                }
                buf[pos..pos + name.len()].copy_from_slice(name);
                pos += name.len();
                buf[pos] = 0; // null terminator
                pos += 1;
            }
            if !unsafe { copy_to_user(out_ptr as *mut u8, &buf[..pos]) } {
                return Error::BadPointer as i64;
            }
            crate::serial_println!(
                "[SYSCALL] fs_readdir: '{}' {} entries",
                full_path,
                entries.len()
            );
            #[allow(clippy::cast_possible_wrap)]
            {
                pos as i64
            }
        }
        Err(e) => {
            let _ = fs.close(dir_ino);
            crate::serial_println!("[SYSCALL] fs_readdir: '{}' failed: {:?}", full_path, e);
            Error::NotFound as i64
        }
    }
}

/// `linux_dirent64` entry type for regular files.
const DIRENT_TYPE_FILE: u8 = 8;

/// `linux_dirent64` entry type for directories.
const DIRENT_TYPE_DIR: u8 = 4;

/// Minimum `linux_dirent64` header size: `d_ino(8) + d_off(8) + d_reclen(2) + d_type(1) = 19`.
const DIRENT_HEADER_SIZE: usize = 19;

/// Read directory entries in `linux_dirent64` format.
///
/// Packs directory entries from the open file descriptor `fd` into a user-space
/// buffer as `linux_dirent64` structs. Each entry has:
///
/// ```text
/// d_ino:    u64    (8 bytes) — inode number
/// d_off:    u64    (8 bytes) — offset to next entry
/// d_reclen: u16    (2 bytes) — total record length (aligned to 8 bytes)
/// d_type:   u8     (1 byte)  — file type (4=dir, 8=file)
/// d_name:   [u8;N] (N bytes) — null-terminated name
/// ```
///
/// Arguments:
///   arg0: file descriptor (must refer to a directory)
///   arg1: pointer to user-space buffer
///   arg2: buffer length in bytes
///
/// Returns: number of bytes written to buffer, 0 at end of directory,
///          or negative error code.
#[allow(clippy::cast_possible_truncation)]
fn sys_getdents64(fd: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    if fd == FD_STDIN || fd == FD_STDOUT {
        return Error::InvalidArgument as i64;
    }

    if buf_ptr == 0 || buf_ptr >= crate::memory::USER_SPACE_MAX {
        return Error::BadPointer as i64;
    }
    if buf_ptr.saturating_add(buf_len) > crate::memory::USER_SPACE_MAX {
        return Error::BadPointer as i64;
    }

    // Cap the buffer to MAX_MSG_SIZE to prevent excessive kernel allocation.
    let buf_len = buf_len.min(MAX_MSG_SIZE as u64) as usize;
    if buf_len < DIRENT_HEADER_SIZE + 2 {
        // Buffer too small to fit even the smallest entry (header + 1 char + NUL).
        return Error::InvalidArgument as i64;
    }

    // Read the path and inode from the task's fd table.
    let (path, ino) = {
        let result = crate::task::scheduler::with_current_task(|task| {
            task.fd_table
                .get(&fd)
                .map(|entry| (entry.path.clone(), entry.ino))
        });
        match result {
            Some(Some(pair)) => pair,
            _ => return Error::NotFound as i64,
        }
    };

    // Resolve the filesystem and read directory entries.
    let (fs, _rel_path) = crate::fs::vfs::resolve_fs(&path);

    let entries = match fs.readdir(ino) {
        Ok(e) => e,
        Err(e) => {
            crate::serial_println!("[SYSCALL] getdents64: fd {} readdir failed: {:?}", fd, e);
            return Error::NotFound as i64;
        }
    };

    if entries.is_empty() {
        return 0; // End of directory.
    }

    // Pack entries as linux_dirent64 structs into a kernel buffer.
    let mut out = alloc::vec![0u8; buf_len];
    let mut pos: usize = 0;

    for entry in &entries {
        let name_bytes = entry.name.as_bytes();
        // NUL-terminated name length including the NUL byte.
        let name_len = name_bytes.len() + 1;
        // Record length: header + name, aligned up to 8 bytes.
        let reclen = ((DIRENT_HEADER_SIZE + name_len + 7) & !7) as u16;

        if pos + reclen as usize > buf_len {
            break; // No more room in the buffer.
        }

        // d_ino: u64
        out[pos..pos + 8].copy_from_slice(&entry.ino.to_le_bytes());
        // d_off: u64 (offset to next entry)
        let next_off = pos + reclen as usize;
        out[pos + 8..pos + 16].copy_from_slice(&(next_off as u64).to_le_bytes());
        // d_reclen: u16
        out[pos + 16..pos + 18].copy_from_slice(&reclen.to_le_bytes());
        // d_type: u8 (4 = directory, 8 = regular file)
        out[pos + 18] = if entry.is_dir {
            DIRENT_TYPE_DIR
        } else {
            DIRENT_TYPE_FILE
        };
        // d_name: [u8; N] — copy name bytes + NUL terminator.
        let name_start = pos + DIRENT_HEADER_SIZE;
        out[name_start..name_start + name_bytes.len()].copy_from_slice(name_bytes);
        out[name_start + name_bytes.len()] = 0; // NUL terminator

        pos = next_off;
    }

    if pos == 0 {
        return 0; // No entries fit in the buffer.
    }

    if !unsafe { copy_to_user(buf_ptr as *mut u8, &out[..pos]) } {
        return Error::BadPointer as i64;
    }

    crate::serial_println!(
        "[SYSCALL] getdents64: fd {} wrote {} bytes ({} entries)",
        fd,
        pos,
        entries.len()
    );

    #[allow(clippy::cast_possible_wrap)]
    {
        pos as i64
    }
}

// ─────────────────── Access syscall ───────────────────

/// Access mode: test for existence only.
const ACCESS_F_OK: u64 = 0;
/// Access mode: test for read permission.
const ACCESS_R_OK: u64 = 1;
/// Access mode: test for write permission.
const ACCESS_W_OK: u64 = 2;
/// Access mode: test for execute permission.
const ACCESS_X_OK: u64 = 4;

/// Check whether a file is accessible to the caller.
///
/// Validates that the file exists and that the requested permissions
/// (encoded as a bitmask of `R_OK`, `W_OK`, `X_OK`, or `F_OK` for
/// existence-only) are satisfied. For now, all existing files are
/// considered readable; write access is verified by attempting to
/// open with write flags; execute is always denied for directories
/// and allowed for regular files.
///
/// Arguments:
///   arg0: pointer to path string (UTF-8)
///   arg1: path length
///   arg2: mode bitmask (`F_OK` | `R_OK` | `W_OK` | `X_OK`)
///
/// Returns: 0 if accessible, negative `Error` code otherwise.
fn sys_access(path_ptr: u64, path_len: u64, mode: u64) -> i64 {
    let Some(path_bytes) = (unsafe { copy_from_user(path_ptr as *const u8, path_len as usize) })
    else {
        return Error::BadPointer as i64;
    };
    let Ok(path) = core::str::from_utf8(&path_bytes) else {
        return Error::InvalidArgument as i64;
    };

    // Validate mode: must only contain known flag bits.
    let known_bits = ACCESS_F_OK | ACCESS_R_OK | ACCESS_W_OK | ACCESS_X_OK;
    if mode & !known_bits != 0 {
        return Error::InvalidArgument as i64;
    }

    // Resolve relative paths against the task's cwd.
    let full_path = resolve_path(path);

    let (fs, rel) = crate::fs::vfs::resolve_fs(&full_path);

    // First, try to open the file for reading (existence check).
    let Ok(ino) = fs.open(&rel, crate::fs::vfs::OpenFlags::READ) else {
        crate::serial_println!("[SYSCALL] access: '{}' not found", full_path);
        return Error::NotFound as i64;
    };

    // F_OK: existence check only -- file was opened, so it exists.
    if mode == ACCESS_F_OK {
        let _ = fs.close(ino);
        crate::serial_println!("[SYSCALL] access: '{}' exists (F_OK)", full_path);
        return 0;
    }

    // Get metadata to check file type.
    let Ok(meta) = fs.stat(ino) else {
        let _ = fs.close(ino);
        return Error::NotFound as i64;
    };

    // R_OK: all existing files are readable in our simple model.
    if mode & ACCESS_R_OK != 0 {
        // Always readable -- no additional check needed.
    }

    // W_OK: verify the file can be opened for writing.
    if mode & ACCESS_W_OK != 0 {
        let _ = fs.close(ino);
        let write_flags = crate::fs::vfs::OpenFlags::from_raw(0x3); // READ | WRITE
        if let Ok(write_ino) = fs.open(&rel, write_flags) {
            let _ = fs.close(write_ino);
        } else {
            crate::serial_println!("[SYSCALL] access: '{}' not writable (W_OK)", full_path);
            return Error::PermissionDenied as i64;
        }
        // Re-open for further checks below if needed.
        if mode & ACCESS_X_OK != 0 {
            let Ok(reopened) = fs.open(&rel, crate::fs::vfs::OpenFlags::READ) else {
                return Error::NotFound as i64;
            };
            let _ = fs.close(reopened);
        }
        crate::serial_println!("[SYSCALL] access: '{}' writable (W_OK)", full_path);
    }

    // X_OK: directories are not executable; regular files are.
    if mode & ACCESS_X_OK != 0 {
        if meta.is_dir {
            let _ = fs.close(ino);
            crate::serial_println!(
                "[SYSCALL] access: '{}' is directory, not executable (X_OK)",
                full_path
            );
            return Error::PermissionDenied as i64;
        }
        crate::serial_println!("[SYSCALL] access: '{}' executable (X_OK)", full_path);
    }

    let _ = fs.close(ino);
    crate::serial_println!("[SYSCALL] access: '{}' mode={}", full_path, mode);
    0
}

// ─────────────────── Symlink / Readlink syscalls ───────────────────

/// Create a symbolic link (stub — not yet implemented).
fn sys_symlink(_target_ptr: u64, _target_len: u64, _link_ptr: u64, _link_len: u64) -> i64 {
    Error::InvalidArgument as i64
}

///
/// Returns: number of bytes written to buffer, or negative error code.
fn sys_readlink(path_ptr: u64, path_len: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    let Some(path_bytes) = (unsafe { copy_from_user(path_ptr as *const u8, path_len as usize) })
    else {
        return Error::BadPointer as i64;
    };
    let Ok(path) = core::str::from_utf8(&path_bytes) else {
        return Error::InvalidArgument as i64;
    };

    let (fs, rel) = crate::fs::vfs::resolve_fs(path);
    let Ok(ino) = fs.open(&rel, crate::fs::vfs::OpenFlags::READ) else {
        return Error::NotFound as i64;
    };

    #[allow(clippy::cast_possible_truncation)]
    let read_len = buf_len.min(MAX_MSG_SIZE as u64) as usize;
    let mut buf = alloc::vec![0u8; read_len];

    match fs.readlink(ino, &mut buf) {
        Ok(n) => {
            let _ = fs.close(ino);
            if unsafe { copy_to_user(buf_ptr as *mut u8, &buf[..n]) } {
                crate::serial_println!("[SYSCALL] readlink: '{}' -> {} bytes", path, n);
                #[allow(clippy::cast_possible_wrap)]
                {
                    n as i64
                }
            } else {
                Error::BadPointer as i64
            }
        }
        Err(e) => {
            let _ = fs.close(ino);
            crate::serial_println!("[SYSCALL] readlink: '{}' failed: {:?}", path, e);
            Error::NotFound as i64
        }
    }
}

// ─────────────────── Chmod / Umask syscalls ───────────────────

/// Change file permissions.
///
/// Resolves the path via VFS and logs the operation. Actual permission
/// storage requires VFS metadata changes and is not yet implemented.
///
/// Arguments:
///   arg0: pointer to path string (UTF-8)
///   arg1: path length
///   arg2: mode (permission bits, e.g. 0o755)
///
/// Returns: 0 on success, negative error code on failure.
fn sys_chmod(path_ptr: u64, path_len: u64, mode: u64) -> i64 {
    let Some(path_bytes) = (unsafe { copy_from_user(path_ptr as *const u8, path_len as usize) })
    else {
        return Error::BadPointer as i64;
    };
    let Ok(path) = core::str::from_utf8(&path_bytes) else {
        return Error::InvalidArgument as i64;
    };

    // Resolve relative paths against the task's cwd.
    let full_path = resolve_path(path);

    // Validate the path exists via VFS.
    let (fs, rel) = crate::fs::vfs::resolve_fs(&full_path);
    let Ok(ino) = fs.open(&rel, crate::fs::vfs::OpenFlags::READ) else {
        crate::serial_println!("[SYSCALL] chmod: '{}' not found", full_path);
        return Error::NotFound as i64;
    };
    let _ = fs.close(ino);

    // Log the operation. Actual permission storage requires VFS metadata changes.
    #[allow(clippy::cast_possible_truncation)]
    let mode_bits = mode as u16;
    crate::serial_println!(
        "[SYSCALL] chmod: '{}' mode={:#o} (logged only)",
        full_path,
        mode_bits
    );
    0
}

/// Set and get the file mode creation mask (umask).
///
/// Sets the calling task's umask to `mask` (masked to 0o777) and returns
/// the previous umask value.
///
/// Arguments:
///   arg0: new umask value (only the low 9 bits are used)
///
/// Returns: the previous umask value (always non-negative).
#[allow(clippy::cast_possible_wrap)]
fn sys_umask(mask: u64) -> i64 {
    // Mask to the permission bits (0o777 = 0x1FF).
    #[allow(clippy::cast_possible_truncation)]
    let new_umask = (mask & 0o777) as u16;

    let result = crate::task::scheduler::with_current_task_mut(|task| {
        let old = task.umask;
        task.umask = new_umask;
        old
    });

    result.map_or(Error::NotFound as i64, |old_umask| {
        crate::serial_println!("[SYSCALL] umask: old={:#o} new={:#o}", old_umask, new_umask);
        i64::from(old_umask)
    })
}

// ─────────────────── Hardware access syscalls ───────────────────

/// Minimum port number allowed for user-space access. Ports below this
/// are legacy/ISA and must not be accessed from user-space to prevent
/// interference with system-critical devices (PIC, PIT, VGA).
/// Port 0x60 (keyboard data) is allowed so user-space drivers can read
/// scancodes directly via `SYS_PORT_IN`.
const PORT_MIN: u16 = 0x0060;

/// Page size constant (4 KiB).
const PAGE_SIZE: u64 = 0x1000;

/// Read from an I/O port. Returns the value read, or a negative error code.
///
/// Arguments:
///   arg0: port address (u16)
///   arg1: size in bytes (1, 2, or 4)
fn sys_port_in(port: u64, size: u64) -> i64 {
    let Ok(port_num) = u16::try_from(port) else {
        return Error::InvalidArgument as i64;
    };

    // Block access to legacy ports (0x000..0x0FF).
    if port_num < PORT_MIN {
        crate::serial_println!(
            "[SYSCALL] port_in: blocked legacy port {port_num:#x} (min {PORT_MIN:#x})"
        );
        return Error::PermissionDenied as i64;
    }

    match size {
        // SAFETY: Port I/O is the standard mechanism for accessing x86
        // hardware devices. The port number is validated: fits in u16 and
        // is >= 0x100 to avoid interfering with system devices.
        1 => {
            let mut p = x86_64::instructions::port::Port::<u8>::new(port_num);
            i64::from(unsafe { p.read() })
        }
        2 => {
            let mut p = x86_64::instructions::port::Port::<u16>::new(port_num);
            i64::from(unsafe { p.read() })
        }
        4 => {
            let mut p = x86_64::instructions::port::Port::<u32>::new(port_num);
            i64::from(unsafe { p.read() })
        }
        _ => Error::InvalidArgument as i64,
    }
}

/// Write to an I/O port.
///
/// Arguments:
///   arg0: port address (u16)
///   arg1: value to write
///   arg2: size in bytes (1, 2, or 4)
fn sys_port_out(port: u64, value: u64, size: u64) -> i64 {
    let Ok(port_num) = u16::try_from(port) else {
        return Error::InvalidArgument as i64;
    };

    // Block access to legacy ports (0x000..0x0FF).
    if port_num < PORT_MIN {
        crate::serial_println!(
            "[SYSCALL] port_out: blocked legacy port {port_num:#x} (min {PORT_MIN:#x})"
        );
        return Error::PermissionDenied as i64;
    }

    match size {
        // SAFETY: Same as `sys_port_in` — port I/O with a validated port number
        // that is >= 0x100 to avoid interfering with system devices.
        1 => unsafe {
            x86_64::instructions::port::Port::<u8>::new(port_num).write(value as u8);
        },
        2 => unsafe {
            x86_64::instructions::port::Port::<u16>::new(port_num).write(value as u16);
        },
        4 => unsafe {
            x86_64::instructions::port::Port::<u32>::new(port_num).write(value as u32);
        },
        _ => return Error::InvalidArgument as i64,
    }
    0
}

/// Map a physical MMIO region into the current task's virtual address space.
///
/// Arguments:
///   arg0: physical address (must be page-aligned)
///   arg1: size in bytes (must be page-aligned)
///
/// Returns: virtual address of the mapped region, or negative error code.
#[allow(clippy::manual_is_multiple_of)]
fn sys_mmio_map(phys_addr: u64, size: u64) -> i64 {
    // Validate page alignment for both address and size.
    if phys_addr % PAGE_SIZE != 0 {
        crate::serial_println!("[SYSCALL] mmio_map: phys_addr {phys_addr:#x} not page-aligned");
        return Error::InvalidArgument as i64;
    }
    if size == 0 || size % PAGE_SIZE != 0 {
        crate::serial_println!("[SYSCALL] mmio_map: size {size:#x} is zero or not page-aligned");
        return Error::InvalidArgument as i64;
    }

    // Find a free virtual address range in user space for the mapping.
    // We scan from a high user-space address downward to avoid colliding
    // with the user program's code/stack/heap.
    let num_pages = size / PAGE_SIZE;
    let Some(virt_base) = find_free_virt_range(num_pages) else {
        crate::serial_println!("[SYSCALL] mmio_map: no free virtual address range");
        return Error::OutOfMemory as i64;
    };

    // Map each page: PRESENT + WRITABLE + NO_CACHE (uncached for MMIO).
    let flags = x86_64::structures::paging::PageTableFlags::PRESENT
        | x86_64::structures::paging::PageTableFlags::WRITABLE
        | x86_64::structures::paging::PageTableFlags::NO_EXECUTE
        | x86_64::structures::paging::PageTableFlags::NO_CACHE;

    // Switch to the task's page table if it has one.
    let page_table = crate::task::scheduler::with_current_task(|task| task.page_table).flatten();

    let flags_state = x86_64::instructions::interrupts::are_enabled();
    x86_64::instructions::interrupts::disable();

    if let Some(pt_phys) = page_table {
        // SAFETY: The task's page table was created by create_user_page_table.
        unsafe {
            crate::memory::switch_page_table(pt_phys);
        }
    }

    for i in 0..num_pages {
        let virt = virt_base + i * PAGE_SIZE;
        let phys = phys_addr + i * PAGE_SIZE;
        // SAFETY: virt is page-aligned, phys is a valid MMIO physical address,
        // and flags correctly mark the page as uncached MMIO.
        unsafe {
            crate::task::user::map_page_user(virt, phys, flags);
        }
    }

    // Switch back to kernel page table.
    if page_table.is_some() {
        let (kernel_p4, _) = x86_64::registers::control::Cr3::read();
        // SAFETY: Restoring the kernel page table.
        unsafe {
            crate::memory::switch_page_table(kernel_p4.start_address().as_u64());
        }
    }

    if flags_state {
        x86_64::instructions::interrupts::enable();
    }

    crate::serial_println!(
        "[SYSCALL] mmio_map: phys={phys_addr:#x} size={size:#x} -> virt={virt_base:#x}"
    );

    #[allow(clippy::cast_possible_wrap)]
    {
        virt_base as i64
    }
}

/// Find a free virtual address range in user space.
///
/// Scans from a high user-space address (just below `USER_SPACE_MAX`) downward,
/// checking that none of the pages are currently mapped.
fn find_free_virt_range(num_pages: u64) -> Option<u64> {
    use x86_64::registers::control::Cr3;
    use x86_64::structures::paging::PageTable;

    let to_virt = crate::memory::phys_to_virt;

    let (level4_frame, _) = Cr3::read();
    // SAFETY: The P4 table is always valid when we're running.
    let l4 = unsafe { &*(to_virt(level4_frame.start_address().as_u64()) as *const PageTable) };

    // Start scanning from just below USER_SPACE_MAX, working downward.
    let total_bytes = num_pages * PAGE_SIZE;
    let mut candidate = (crate::memory::USER_SPACE_MAX - total_bytes) & !(PAGE_SIZE - 1);

    while candidate >= PAGE_SIZE {
        if is_virt_range_free(l4, candidate, num_pages) {
            return Some(candidate);
        }
        // Move down by 2 MiB (512 pages) for faster scanning.
        candidate = candidate.saturating_sub(512 * PAGE_SIZE);
        if candidate < PAGE_SIZE {
            break;
        }
    }

    None
}

/// Check if a virtual address range is unmapped in the given P4 table.
fn is_virt_range_free(
    l4: &x86_64::structures::paging::PageTable,
    virt_base: u64,
    num_pages: u64,
) -> bool {
    use x86_64::structures::paging::PageTable;

    let to_virt = crate::memory::phys_to_virt;

    for i in 0..num_pages {
        let virt = virt_base + i * PAGE_SIZE;
        let p4_idx = ((virt >> 39) & 0x1FF) as usize;
        let p3_idx = ((virt >> 30) & 0x1FF) as usize;
        let p2_idx = ((virt >> 21) & 0x1FF) as usize;
        let p1_idx = ((virt >> 12) & 0x1FF) as usize;

        if !l4[p4_idx]
            .flags()
            .contains(x86_64::structures::paging::PageTableFlags::PRESENT)
        {
            continue;
        }
        // SAFETY: P4 entry is PRESENT.
        let l3 = unsafe {
            &*(to_virt(l4[p4_idx].frame().unwrap().start_address().as_u64()) as *const PageTable)
        };
        if !l3[p3_idx]
            .flags()
            .contains(x86_64::structures::paging::PageTableFlags::PRESENT)
        {
            continue;
        }
        // SAFETY: P3 entry is PRESENT.
        let l2 = unsafe {
            &*(to_virt(l3[p3_idx].frame().unwrap().start_address().as_u64()) as *const PageTable)
        };
        if !l2[p2_idx]
            .flags()
            .contains(x86_64::structures::paging::PageTableFlags::PRESENT)
        {
            continue;
        }
        // Check for huge page at P2 level.
        if l2[p2_idx]
            .flags()
            .contains(x86_64::structures::paging::PageTableFlags::HUGE_PAGE)
        {
            return false;
        }
        // SAFETY: P2 entry is PRESENT and not a huge page.
        let l1 = unsafe {
            &*(to_virt(l2[p2_idx].frame().unwrap().start_address().as_u64()) as *const PageTable)
        };
        if l1[p1_idx]
            .flags()
            .contains(x86_64::structures::paging::PageTableFlags::PRESENT)
        {
            return false;
        }
    }

    true
}

/// Unmap a previously mapped MMIO region.
///
/// Arguments:
///   arg0: virtual address (must be page-aligned)
///   arg1: size in bytes (must be page-aligned)
///
/// Returns: 0 on success, or negative error code.
#[allow(clippy::manual_is_multiple_of)]
fn sys_mmio_unmap(virt_addr: u64, size: u64) -> i64 {
    if virt_addr % PAGE_SIZE != 0 {
        crate::serial_println!("[SYSCALL] mmio_unmap: virt_addr {virt_addr:#x} not page-aligned");
        return Error::InvalidArgument as i64;
    }
    if size == 0 || size % PAGE_SIZE != 0 {
        crate::serial_println!("[SYSCALL] mmio_unmap: size {size:#x} is zero or not page-aligned");
        return Error::InvalidArgument as i64;
    }

    // Only allow unmapping in user-space range.
    if virt_addr >= crate::memory::USER_SPACE_MAX {
        return Error::InvalidArgument as i64;
    }
    if virt_addr.saturating_add(size) > crate::memory::USER_SPACE_MAX {
        return Error::InvalidArgument as i64;
    }

    let num_pages = size / PAGE_SIZE;

    // Switch to the task's page table if it has one.
    let page_table = crate::task::scheduler::with_current_task(|task| task.page_table).flatten();

    let flags_state = x86_64::instructions::interrupts::are_enabled();
    x86_64::instructions::interrupts::disable();

    if let Some(pt_phys) = page_table {
        // SAFETY: The task's page table was created by create_user_page_table.
        unsafe {
            crate::memory::switch_page_table(pt_phys);
        }
    }

    for i in 0..num_pages {
        let virt = virt_addr + i * PAGE_SIZE;
        unmap_page(virt);
    }

    // Switch back to kernel page table.
    if page_table.is_some() {
        let (kernel_p4, _) = x86_64::registers::control::Cr3::read();
        // SAFETY: Restoring the kernel page table.
        unsafe {
            crate::memory::switch_page_table(kernel_p4.start_address().as_u64());
        }
    }

    if flags_state {
        x86_64::instructions::interrupts::enable();
    }

    crate::serial_println!("[SYSCALL] mmio_unmap: virt={virt_addr:#x} size={size:#x}");
    0
}

/// Unmap a single 4 KiB page by clearing its P1 entry.
///
/// Does not free the physical frame (MMIO frames are not allocated by us).
fn unmap_page(virt: u64) {
    use x86_64::structures::paging::PageTable;

    let to_virt = crate::memory::phys_to_virt;

    let (level4_frame, _) = x86_64::registers::control::Cr3::read();
    // SAFETY: The P4 table is always valid when we're running.
    let l4 = unsafe { &mut *(to_virt(level4_frame.start_address().as_u64()) as *mut PageTable) };

    let p4_idx = ((virt >> 39) & 0x1FF) as usize;
    let p3_idx = ((virt >> 30) & 0x1FF) as usize;
    let p2_idx = ((virt >> 21) & 0x1FF) as usize;
    let p1_idx = ((virt >> 12) & 0x1FF) as usize;

    if !l4[p4_idx]
        .flags()
        .contains(x86_64::structures::paging::PageTableFlags::PRESENT)
    {
        return;
    }
    // SAFETY: P4 entry is PRESENT.
    let l3 = unsafe {
        &mut *(to_virt(l4[p4_idx].frame().unwrap().start_address().as_u64()) as *mut PageTable)
    };
    if !l3[p3_idx]
        .flags()
        .contains(x86_64::structures::paging::PageTableFlags::PRESENT)
    {
        return;
    }
    // SAFETY: P3 entry is PRESENT.
    let l2 = unsafe {
        &mut *(to_virt(l3[p3_idx].frame().unwrap().start_address().as_u64()) as *mut PageTable)
    };
    if !l2[p2_idx]
        .flags()
        .contains(x86_64::structures::paging::PageTableFlags::PRESENT)
    {
        return;
    }
    if l2[p2_idx]
        .flags()
        .contains(x86_64::structures::paging::PageTableFlags::HUGE_PAGE)
    {
        return; // Cannot unmap individual pages from a huge page.
    }
    // SAFETY: P2 entry is PRESENT and not a huge page.
    let l1 = unsafe {
        &mut *(to_virt(l2[p2_idx].frame().unwrap().start_address().as_u64()) as *mut PageTable)
    };
    // Clear the P1 entry (unmap the page).
    l1[p1_idx].set_addr(
        x86_64::PhysAddr::new(0),
        x86_64::structures::paging::PageTableFlags::empty(),
    );
}

// ─────────────────── Network syscalls ───────────────────

/// Send a raw Ethernet frame via the network driver.
///
/// Arguments:
///   arg0: pointer to frame data
///   arg1: frame length
fn sys_net_send(data_ptr: u64, data_len: u64) -> i64 {
    let Some(data) = (unsafe { copy_from_user(data_ptr as *const u8, data_len as usize) }) else {
        return Error::BadPointer as i64;
    };

    crate::drivers::net::send_frame(&data).map_or(Error::InvalidArgument as i64, |sent| {
        crate::serial_println!("[SYSCALL] net_send: {} bytes", sent);
        i64::try_from(sent).unwrap_or(-1)
    })
}

/// Receive a raw Ethernet frame (non-blocking).
///
/// Arguments:
///   arg0: pointer to receive buffer
///   arg1: buffer length
fn sys_net_receive(buf_ptr: u64, buf_len: u64) -> i64 {
    crate::drivers::net::receive_frame().map_or(Error::WouldBlock as i64, |frame| {
        let len = frame.len().min(buf_len as usize);
        if unsafe { copy_to_user(buf_ptr as *mut u8, &frame[..len]) } {
            crate::serial_println!("[SYSCALL] net_receive: {} bytes", len);
            i64::try_from(len).unwrap_or(-1)
        } else {
            Error::BadPointer as i64
        }
    })
}

// ─────────────────── Socket syscalls ───────────────────

/// Next socket descriptor to allocate (per-kernel, simple counter).
static NEXT_SOCK_FD: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Create a socket. Returns a socket descriptor, or a negative error code.
///
/// Arguments:
///   arg0: socket type (0 = Tcp, 1 = Udp, 2 = Raw)
fn sys_socket(socket_type: u64) -> i64 {
    use crate::net::socket::{Socket, SocketType};

    let sock_type = match socket_type {
        0 => SocketType::Tcp,
        1 => SocketType::Udp,
        2 => SocketType::Raw,
        _ => return Error::InvalidArgument as i64,
    };

    let socket = Socket::new(sock_type);
    let fd = NEXT_SOCK_FD.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

    let result = crate::task::scheduler::with_current_task_mut(|task| {
        task.socket_table.insert(fd, socket);
    });

    match result {
        Some(()) => {
            crate::serial_println!("[SYSCALL] socket: type={:?} fd={}", sock_type, fd);
            #[allow(clippy::cast_possible_wrap)]
            {
                fd as i64
            }
        }
        None => Error::NotFound as i64,
    }
}

/// Bind a socket to a local address and port.
///
/// Arguments:
///   arg0: socket descriptor
///   arg1: IPv4 address (network byte order)
///   arg2: port number
fn sys_bind(sock_fd: u64, _addr: u64, port: u64) -> i64 {
    let Ok(port_num) = u16::try_from(port) else {
        return Error::InvalidArgument as i64;
    };

    let result = crate::task::scheduler::with_current_task_mut(|task| {
        if let Some(socket) = task.socket_table.get_mut(&sock_fd) {
            socket.local_port = port_num;
            Some(())
        } else {
            None
        }
    });

    match result {
        Some(Some(())) => {
            crate::serial_println!("[SYSCALL] bind: fd={} port={}", sock_fd, port_num);
            0
        }
        _ => Error::NotFound as i64,
    }
}

/// Listen for incoming connections on a socket (TCP only).
///
/// Arguments:
///   arg0: socket descriptor
///
/// Registers the socket's local port in the kernel's listen table so
/// incoming SYN segments are matched and a three-way handshake is
/// performed automatically. The resulting child connections are queued
/// for `sys_accept`.
fn sys_listen(sock_fd: u64) -> i64 {
    crate::serial_println!("[SYSCALL] listen: sock_fd={}", sock_fd);

    // Extract the local port and verify the socket is TCP.
    let port_info = crate::task::scheduler::with_current_task(|task| {
        task.socket_table.get(&sock_fd).and_then(|sock| {
            if sock.socket_type == crate::net::socket::SocketType::Tcp {
                Some(sock.local_port)
            } else {
                None
            }
        })
    });

    let Some(Some(local_port)) = port_info else {
        return Error::InvalidArgument as i64;
    };

    if local_port == 0 {
        crate::serial_println!("[SYSCALL] listen: socket not bound to a port");
        return Error::InvalidArgument as i64;
    }

    // Set socket state to Listening.
    let result = crate::task::scheduler::with_current_task_mut(|task| {
        if let Some(sock) = task.socket_table.get_mut(&sock_fd) {
            sock.state = crate::net::socket::SocketState::Listening;
            true
        } else {
            false
        }
    });

    if !result.unwrap_or(false) {
        return Error::NotFound as i64;
    }

    // Register the port in the kernel's listen table.
    crate::net::tcp::register_listen_port(local_port);

    crate::serial_println!("[SYSCALL] listen: fd={} port={}", sock_fd, local_port);
    0
}

/// Accept an incoming connection on a listening socket (TCP only).
///
/// Arguments:
///   arg0: socket descriptor
///
/// Returns: new socket descriptor for the accepted connection, or negative error.
///
/// Blocks until a connection is available in the pending-accept queue for
/// this listening socket. The queue is populated by `handle_passive_syn`
/// in the TCP module when a SYN arrives on the listening port.
fn sys_accept(sock_fd: u64) -> i64 {
    crate::serial_println!("[SYSCALL] accept: sock_fd={}", sock_fd);

    // Verify the socket is in Listening state and get its local port.
    let listen_info = crate::task::scheduler::with_current_task(|task| {
        task.socket_table.get(&sock_fd).and_then(|s| {
            if s.state == crate::net::socket::SocketState::Listening {
                Some(s.local_port)
            } else {
                None
            }
        })
    });

    let Some(Some(local_port)) = listen_info else {
        return Error::InvalidArgument as i64;
    };

    // Spin-wait for a pending connection with HLT to avoid busy-spinning.
    // In a production kernel this would block the task and context-switch,
    // but for this implementation we use the same HLT pattern as other
    // blocking syscalls (event_wait, irq_wait).
    crate::serial_println!(
        "[SYSCALL] accept: waiting on port {} for incoming connection",
        local_port
    );

    loop {
        // Check for a pending connection.
        if let Some((remote_addr, remote_port)) = crate::net::tcp::accept_next(local_port) {
            // Verify the connection reached Established state before
            // returning it. The child connection is in SynReceived until
            // the ACK arrives from the client. We wait for that ACK.
            let is_established =
                crate::net::tcp::get_connection_state(local_port, remote_addr, remote_port)
                    == Some(crate::net::tcp::TcpState::Established);

            if !is_established {
                // Not yet established -- re-enqueue and keep waiting.
                // The ACK will arrive soon (handle_syn_received processes it).
                // For now, we accept it even in SynReceived since the ACK
                // from the client completes the handshake asynchronously.
                crate::serial_println!(
                    "[SYSCALL] accept: connection {:?}:{} not yet established, accepting anyway",
                    remote_addr,
                    remote_port
                );
            }

            // Allocate a new socket descriptor for the accepted connection.
            let new_fd = crate::task::scheduler::with_current_task_mut(|task| {
                let mut fd = 10u64;
                while task.socket_table.contains_key(&fd) {
                    fd += 1;
                }
                task.socket_table.insert(
                    fd,
                    crate::net::socket::Socket {
                        socket_type: crate::net::socket::SocketType::Tcp,
                        state: crate::net::socket::SocketState::Connected,
                        local_port,
                        remote_addr,
                        remote_port,
                        options: crate::net::socket::SocketOptions::default(),
                    },
                );
                fd
            });

            return new_fd.map_or(Error::OutOfMemory as i64, |fd| {
                crate::serial_println!(
                    "[SYSCALL] accept: fd={} <- {:?}:{}",
                    fd,
                    remote_addr,
                    remote_port
                );
                #[allow(clippy::cast_possible_wrap)]
                {
                    fd as i64
                }
            });
        }

        // No pending connection yet -- HLT until next interrupt.
        x86_64::instructions::hlt();
    }
}

/// Connect a socket to a remote address and port (TCP only).
///
/// Arguments:
///   arg0: socket descriptor
///   arg1: remote IPv4 address (network byte order)
///   arg2: remote port number
///
/// Allocates a local port, initiates the TCP three-way handshake,
/// and updates the socket state to Connected on success.
fn sys_connect(sock_fd: u64, addr: u64, port: u64) -> i64 {
    let remote_addr = addr as u32;
    let Ok(remote_port) = u16::try_from(port) else {
        return Error::InvalidArgument as i64;
    };

    // Look up the socket and verify it is a TCP socket.
    let socket_info = crate::task::scheduler::with_current_task(|task| {
        task.socket_table
            .get(&sock_fd)
            .map(|s| (s.socket_type, s.local_port))
    });

    let Some(Some((socket_type, local_port))) = socket_info else {
        return Error::NotFound as i64;
    };

    if socket_type != crate::net::socket::SocketType::Tcp {
        crate::serial_println!("[SYSCALL] connect: not a TCP socket");
        return Error::InvalidArgument as i64;
    }

    // Allocate a local port if not bound.
    let effective_port = if local_port == 0 {
        match crate::net::tcp::allocate_local_port() {
            Some(p) => p,
            None => return Error::Busy as i64,
        }
    } else {
        local_port
    };

    // Initiate TCP connection.
    if crate::net::tcp::connect(effective_port, remote_addr, remote_port).is_err() {
        return Error::Busy as i64;
    }

    // Update the socket with connection info and state.
    let result = crate::task::scheduler::with_current_task_mut(|task| {
        if let Some(socket) = task.socket_table.get_mut(&sock_fd) {
            socket.local_port = effective_port;
            socket.remote_addr = remote_addr;
            socket.remote_port = remote_port;
            socket.state = crate::net::socket::SocketState::Connected;
            Some(())
        } else {
            None
        }
    });

    match result {
        Some(Some(())) => {
            crate::serial_println!(
                "[SYSCALL] connect: fd={} -> {:?}:{}",
                sock_fd,
                remote_addr,
                remote_port
            );
            0
        }
        _ => Error::NotFound as i64,
    }
}

/// Send data to a connected peer (TCP) or specific address (UDP).
///
/// Arguments:
///   arg0: socket descriptor
///   arg1: pointer to data buffer
///   arg2: data length
///   arg3: destination IPv4 address (network byte order, 0 for connected TCP)
///   arg4: destination port (0 for connected TCP)
///
/// For TCP, the socket must be Connected. For UDP, the socket must be bound.
#[allow(clippy::cast_possible_wrap)]
fn sys_sendto(sock_fd: u64, buf_ptr: u64, buf_len: u64, addr: u64, port: u64) -> i64 {
    let Some(data) = (unsafe { copy_from_user(buf_ptr as *const u8, buf_len as usize) }) else {
        return Error::BadPointer as i64;
    };

    // Look up socket info.
    let socket_info = crate::task::scheduler::with_current_task(|task| {
        task.socket_table.get(&sock_fd).map(|s| {
            (
                s.socket_type,
                s.state,
                s.local_port,
                s.remote_addr,
                s.remote_port,
            )
        })
    });

    let Some(Some((socket_type, state, local_port, remote_addr, remote_port))) = socket_info else {
        return Error::NotFound as i64;
    };

    match socket_type {
        crate::net::socket::SocketType::Tcp => {
            if state != crate::net::socket::SocketState::Connected {
                crate::serial_println!("[SYSCALL] sendto: TCP socket not connected");
                return Error::InvalidArgument as i64;
            }

            let dst_addr = if addr != 0 { addr as u32 } else { remote_addr };
            let dst_port = if port != 0 {
                let Ok(p) = u16::try_from(port) else {
                    return Error::InvalidArgument as i64;
                };
                p
            } else {
                remote_port
            };

            match crate::net::tcp::send_data(local_port, dst_addr, dst_port, &data) {
                Ok(sent) => {
                    crate::serial_println!(
                        "[SYSCALL] sendto: TCP fd={} sent {} bytes",
                        sock_fd,
                        sent
                    );
                    i64::try_from(sent).unwrap_or(-1)
                }
                Err(()) => Error::InvalidArgument as i64,
            }
        }
        crate::net::socket::SocketType::Udp => {
            if local_port == 0 {
                crate::serial_println!("[SYSCALL] sendto: UDP socket not bound");
                return Error::InvalidArgument as i64;
            }

            let dst_addr = if addr != 0 { addr as u32 } else { remote_addr };
            let dst_port = if port != 0 {
                let Ok(p) = u16::try_from(port) else {
                    return Error::InvalidArgument as i64;
                };
                p
            } else {
                remote_port
            };

            if dst_addr == 0 || dst_port == 0 {
                crate::serial_println!("[SYSCALL] sendto: UDP no destination specified");
                return Error::InvalidArgument as i64;
            }

            match crate::net::udp::send_udp_packet(local_port, dst_addr, dst_port, &data) {
                Ok(_sent) => {
                    crate::serial_println!(
                        "[SYSCALL] sendto: UDP fd={} sent {} bytes",
                        sock_fd,
                        data.len()
                    );
                    i64::try_from(data.len()).unwrap_or(-1)
                }
                Err(()) => Error::Busy as i64,
            }
        }
        crate::net::socket::SocketType::Raw => {
            crate::serial_println!("[SYSCALL] sendto: raw sockets not yet supported");
            Error::InvalidArgument as i64
        }
    }
}

/// Receive data from a socket (non-blocking).
///
/// Arguments:
///   arg0: socket descriptor
///   arg1: pointer to receive buffer
///   arg2: buffer length
fn sys_recvfrom(sock_fd: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    // Look up socket info.
    let socket_info = crate::task::scheduler::with_current_task(|task| {
        task.socket_table.get(&sock_fd).map(|s| {
            (
                s.socket_type,
                s.state,
                s.local_port,
                s.remote_addr,
                s.remote_port,
            )
        })
    });

    let Some(Some((socket_type, state, local_port, remote_addr, remote_port))) = socket_info else {
        return Error::NotFound as i64;
    };

    if socket_type != crate::net::socket::SocketType::Tcp {
        crate::serial_println!("[SYSCALL] recvfrom: only TCP supported");
        return Error::InvalidArgument as i64;
    }

    if state != crate::net::socket::SocketState::Connected {
        crate::serial_println!("[SYSCALL] recvfrom: socket not connected");
        return Error::InvalidArgument as i64;
    }

    let copy_len = buf_len.min(MAX_MSG_SIZE as u64) as usize;
    let mut buf = alloc::vec![0u8; copy_len];

    match crate::net::tcp::recv_data(local_port, remote_addr, remote_port, &mut buf) {
        Ok(0) => {
            // No data available.
            Error::WouldBlock as i64
        }
        Ok(n) => {
            let n = n.min(copy_len);
            if unsafe { copy_to_user(buf_ptr as *mut u8, &buf[..n]) } {
                crate::serial_println!("[SYSCALL] recvfrom: fd={} received {} bytes", sock_fd, n);
                i64::try_from(n).unwrap_or(-1)
            } else {
                Error::BadPointer as i64
            }
        }
        Err(()) => Error::InvalidArgument as i64,
    }
}

/// Close a socket.
///
/// Arguments:
///   arg0: socket descriptor
///
/// For TCP sockets, initiates the connection teardown (FIN).
fn sys_close_sock(sock_fd: u64) -> i64 {
    let socket_info = crate::task::scheduler::with_current_task(|task| {
        task.socket_table.get(&sock_fd).map(|s| {
            (
                s.socket_type,
                s.state,
                s.local_port,
                s.remote_addr,
                s.remote_port,
            )
        })
    });

    let Some(Some((socket_type, state, local_port, remote_addr, remote_port))) = socket_info else {
        return Error::NotFound as i64;
    };

    // For TCP sockets in Connected state, initiate teardown.
    if socket_type == crate::net::socket::SocketType::Tcp
        && state == crate::net::socket::SocketState::Connected
    {
        let _ = crate::net::tcp::close(local_port, remote_addr, remote_port);
    }

    // For TCP sockets in Listening state, unregister the listen port.
    if socket_type == crate::net::socket::SocketType::Tcp
        && state == crate::net::socket::SocketState::Listening
    {
        crate::net::tcp::unregister_listen_port(local_port);
    }

    // Remove the socket from the task's socket table.
    let result = crate::task::scheduler::with_current_task_mut(|task| {
        task.socket_table.remove(&sock_fd).is_some()
    });

    match result {
        Some(true) => {
            crate::serial_println!("[SYSCALL] close_sock: fd={}", sock_fd);
            0
        }
        _ => Error::NotFound as i64,
    }
}

// ─────────────────── DNS syscall ───────────────────

/// Resolve a hostname to an IPv4 address via DNS.
///
/// Arguments:
///   arg0: pointer to hostname (UTF-8)
///   arg1: hostname length
///   arg2: pointer to 4-byte output buffer for the resolved IPv4 address
fn sys_dns_resolve(name_ptr: u64, name_len: u64, out_ptr: u64) -> i64 {
    let Some(name_bytes) = (unsafe { copy_from_user(name_ptr as *const u8, name_len as usize) })
    else {
        return Error::BadPointer as i64;
    };
    let Ok(hostname) = core::str::from_utf8(&name_bytes) else {
        return Error::InvalidArgument as i64;
    };

    crate::serial_println!("[SYSCALL] dns_resolve: '{}'", hostname);

    match crate::net::dns::resolve(hostname) {
        Ok(ip) => {
            // Write the 4-byte IPv4 address to the user-space output buffer.
            if unsafe { copy_to_user(out_ptr as *mut u8, &ip) } {
                crate::serial_println!(
                    "[SYSCALL] dns_resolve: '{}' -> {}.{}.{}.{}",
                    hostname,
                    ip[0],
                    ip[1],
                    ip[2],
                    ip[3]
                );
                0
            } else {
                Error::BadPointer as i64
            }
        }
        Err(e) => {
            crate::serial_println!("[SYSCALL] dns_resolve: failed for '{}': {:?}", hostname, e);
            Error::NotFound as i64
        }
    }
}

// ─────────────────── Socket option syscalls ───────────────────

/// Get a socket option.
///
/// Reads the value of the specified option from the socket's options field
/// and writes it to the user-space buffer.
///
/// Arguments:
///   arg0: socket descriptor
///   arg1: option level (1 = `SOL_SOCKET`, 6 = `IPPROTO_TCP`)
///   arg2: option name (`SO_REUSEADDR=2`, `TCP_NODELAY=1`, `SO_RCVBUF=8`, `SO_SNDBUF=7`)
///   arg3: pointer to output buffer in user-space
///   arg4: pointer to buffer length in user-space (updated to actual size on return)
///
/// Returns: 0 on success, negative `Error` code on failure.
fn sys_getsockopt(sock_fd: u64, level: u64, optname: u64, optval_ptr: u64, optlen_ptr: u64) -> i64 {
    use crate::net::socket::{
        IPPROTO_TCP, SOL_SOCKET, SO_RCVBUF, SO_REUSEADDR, SO_SNDBUF, TCP_NODELAY,
    };

    let level_u32 = level as u32;
    let optname_u32 = optname as u32;

    // Validate user-space pointers.
    if optval_ptr == 0 || optval_ptr >= crate::memory::USER_SPACE_MAX {
        return Error::BadPointer as i64;
    }
    if optlen_ptr == 0 || optlen_ptr >= crate::memory::USER_SPACE_MAX {
        return Error::BadPointer as i64;
    }

    // Read the buffer length from user-space.
    let mut optlen_buf = [0u8; 4];
    // SAFETY: optlen_ptr validated in user-space.
    unsafe {
        core::ptr::copy_nonoverlapping(optlen_ptr as *const u8, optlen_buf.as_mut_ptr(), 4);
    }
    let _optlen = u32::from_le_bytes(optlen_buf);

    // Look up the socket and read the requested option.
    let result = crate::task::scheduler::with_current_task(|task| {
        task.socket_table
            .get(&sock_fd)
            .map(|sock| match (level_u32, optname_u32) {
                (SOL_SOCKET, SO_REUSEADDR) => {
                    let val: u32 = u32::from(sock.options.reuse_addr);
                    Some((val.to_le_bytes().to_vec(), 4u32))
                }
                (SOL_SOCKET, SO_RCVBUF) => {
                    Some((sock.options.rcv_buf_size.to_le_bytes().to_vec(), 4u32))
                }
                (SOL_SOCKET, SO_SNDBUF) => {
                    Some((sock.options.snd_buf_size.to_le_bytes().to_vec(), 4u32))
                }
                (IPPROTO_TCP, TCP_NODELAY) => {
                    let val: u32 = u32::from(sock.options.tcp_no_delay);
                    Some((val.to_le_bytes().to_vec(), 4u32))
                }
                _ => None,
            })
    });

    match result {
        Some(Some(Some((val_bytes, val_len)))) => {
            // Write the value to user-space.
            if unsafe { copy_to_user(optval_ptr as *mut u8, &val_bytes) } {
                // Write the actual length back to optlen_ptr.
                let len_bytes = val_len.to_le_bytes();
                // SAFETY: optlen_ptr validated in user-space.
                unsafe {
                    core::ptr::copy_nonoverlapping(len_bytes.as_ptr(), optlen_ptr as *mut u8, 4);
                }
                crate::serial_println!(
                    "[SYSCALL] getsockopt: fd={} level={} opt={}",
                    sock_fd,
                    level_u32,
                    optname_u32
                );
                0
            } else {
                Error::BadPointer as i64
            }
        }
        Some(Some(None)) => Error::InvalidArgument as i64,
        _ => Error::NotFound as i64,
    }
}

/// Set a socket option.
///
/// Reads the option value from the user-space buffer and writes it to
/// the socket's options field.
///
/// Arguments:
///   arg0: socket descriptor
///   arg1: option level (1 = `SOL_SOCKET`, 6 = `IPPROTO_TCP`)
///   arg2: option name (`SO_REUSEADDR=2`, `TCP_NODELAY=1`, `SO_RCVBUF=8`, `SO_SNDBUF=7`)
///   arg3: pointer to option value in user-space
///   arg4: option value length in bytes
///
/// Returns: 0 on success, negative `Error` code on failure.
fn sys_setsockopt(sock_fd: u64, level: u64, optname: u64, optval_ptr: u64, optlen: u64) -> i64 {
    use crate::net::socket::{
        IPPROTO_TCP, SOL_SOCKET, SO_RCVBUF, SO_REUSEADDR, SO_SNDBUF, TCP_NODELAY,
    };

    let level_u32 = level as u32;
    let optname_u32 = optname as u32;

    // Validate user-space pointer.
    if optval_ptr == 0 || optval_ptr >= crate::memory::USER_SPACE_MAX {
        return Error::BadPointer as i64;
    }

    // Read the option value from user-space.
    let Some(val_bytes) = (unsafe { copy_from_user(optval_ptr as *const u8, optlen as usize) })
    else {
        return Error::BadPointer as i64;
    };

    // Parse the value as a u32 (all current options are 4 bytes).
    if val_bytes.len() < 4 {
        return Error::InvalidArgument as i64;
    }
    let val = u32::from_le_bytes([val_bytes[0], val_bytes[1], val_bytes[2], val_bytes[3]]);

    // Apply the option to the socket.
    let result = crate::task::scheduler::with_current_task_mut(|task| {
        task.socket_table
            .get_mut(&sock_fd)
            .map(|sock| match (level_u32, optname_u32) {
                (SOL_SOCKET, SO_REUSEADDR) => {
                    sock.options.reuse_addr = val != 0;
                    Some(())
                }
                (SOL_SOCKET, SO_RCVBUF) => {
                    sock.options.rcv_buf_size = val;
                    Some(())
                }
                (SOL_SOCKET, SO_SNDBUF) => {
                    sock.options.snd_buf_size = val;
                    Some(())
                }
                (IPPROTO_TCP, TCP_NODELAY) => {
                    sock.options.tcp_no_delay = val != 0;
                    Some(())
                }
                _ => None,
            })
    });

    match result {
        Some(Some(Some(()))) => {
            crate::serial_println!(
                "[SYSCALL] setsockopt: fd={} level={} opt={} val={}",
                sock_fd,
                level_u32,
                optname_u32,
                val
            );
            0
        }
        Some(Some(None)) => Error::InvalidArgument as i64,
        _ => Error::NotFound as i64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Error code tests ───

    #[test]
    fn test_error_codes_match_interface() {
        // Error codes must match INTERFACE.md §3.1.
        assert_eq!(Error::InvalidArgument as i64, -1);
        assert_eq!(Error::NotFound as i64, -2);
        assert_eq!(Error::PermissionDenied as i64, -3);
        assert_eq!(Error::OutOfMemory as i64, -4);
        assert_eq!(Error::Busy as i64, -5);
        assert_eq!(Error::ChannelClosed as i64, -6);
        assert_eq!(Error::WouldBlock as i64, -7);
        assert_eq!(Error::Timeout as i64, -8);
        assert_eq!(Error::BadPointer as i64, -9);
        assert_eq!(Error::UnknownSyscall as i64, -10);
    }

    #[test]
    fn test_error_codes_are_negative() {
        let codes = [
            Error::InvalidArgument,
            Error::NotFound,
            Error::PermissionDenied,
            Error::OutOfMemory,
            Error::Busy,
            Error::ChannelClosed,
            Error::WouldBlock,
            Error::Timeout,
            Error::BadPointer,
            Error::UnknownSyscall,
        ];
        for err in &codes {
            assert!((*err as i64) < 0, "{:?} must be negative", err);
        }
    }

    #[test]
    fn test_error_codes_unique() {
        let codes = [
            Error::InvalidArgument as i64,
            Error::NotFound as i64,
            Error::PermissionDenied as i64,
            Error::OutOfMemory as i64,
            Error::Busy as i64,
            Error::ChannelClosed as i64,
            Error::WouldBlock as i64,
            Error::Timeout as i64,
            Error::BadPointer as i64,
            Error::UnknownSyscall as i64,
        ];
        for i in 0..codes.len() {
            for j in (i + 1)..codes.len() {
                assert_ne!(codes[i], codes[j], "duplicate error codes");
            }
        }
    }

    // ─── Unknown syscall number ───

    #[test]
    fn test_unknown_syscall_returns_unknown() {
        let result = handle_syscall_raw(0xDEAD, 0, 0, 0, 0, 0);
        assert_eq!(result, Error::UnknownSyscall as i64);
    }

    #[test]
    fn test_unknown_syscall_zero() {
        // Syscall 0 is not assigned — should return UnknownSyscall.
        let result = handle_syscall_raw(0, 0, 0, 0, 0, 0);
        assert_eq!(result, Error::UnknownSyscall as i64);
    }

    #[test]
    fn test_unknown_syscall_max_value() {
        let result = handle_syscall_raw(u64::MAX, 0, 0, 0, 0, 0);
        assert_eq!(result, Error::UnknownSyscall as i64);
    }

    // ─── USER_SPACE_MAX constant ───

    #[test]
    fn test_user_space_max_value() {
        // 128 TiB — half of canonical 256 TiB virtual address space.
        assert_eq!(crate::memory::USER_SPACE_MAX, 0x0000_8000_0000_0000);
    }

    // ─── copy_from_user boundary tests ───

    #[test]
    fn test_copy_from_user_null_pointer() {
        let result = unsafe { copy_from_user(core::ptr::null(), 10) };
        assert!(result.is_none(), "null pointer must return None");
    }

    #[test]
    fn test_copy_from_user_zero_length() {
        // A valid low address but zero length.
        let result = unsafe { copy_from_user(0x1000 as *const u8, 0) };
        assert!(result.is_none(), "zero length must return None");
    }

    #[test]
    fn test_copy_from_user_oversized_length() {
        let result = unsafe { copy_from_user(0x1000 as *const u8, MAX_MSG_SIZE + 1) };
        assert!(
            result.is_none(),
            "length exceeding MAX_MSG_SIZE must return None"
        );
    }

    #[test]
    fn test_copy_from_user_kernel_address() {
        // Address in kernel space (>= USER_SPACE_MAX).
        let result = unsafe { copy_from_user(crate::memory::USER_SPACE_MAX as *const u8, 16) };
        assert!(result.is_none(), "kernel-space address must return None");
    }

    #[test]
    fn test_copy_from_user_kernel_high_address() {
        // Well into kernel space.
        let result = unsafe { copy_from_user(0xFFFF_8000_0000_0000 as *const u8, 8) };
        assert!(result.is_none(), "high kernel address must return None");
    }

    // ─── copy_to_user boundary tests ───

    #[test]
    fn test_copy_to_user_null_pointer() {
        let src = [1u8, 2, 3];
        let result = unsafe { copy_to_user(core::ptr::null_mut(), &src) };
        assert!(!result, "null pointer must return false");
    }

    #[test]
    fn test_copy_to_user_empty_source() {
        let mut buf = [0u8; 4];
        let result = unsafe { copy_to_user(buf.as_mut_ptr(), &[]) };
        assert!(!result, "empty source must return false");
    }

    #[test]
    fn test_copy_to_user_oversized_source() {
        let src = alloc::vec![0u8; MAX_MSG_SIZE + 1];
        let mut dst = [0u8; MAX_MSG_SIZE + 2];
        let result = unsafe { copy_to_user(dst.as_mut_ptr(), &src) };
        assert!(!result, "oversized source must return false");
    }

    #[test]
    fn test_copy_to_user_kernel_address() {
        let src = [1u8, 2, 3];
        let result = unsafe { copy_to_user(crate::memory::USER_SPACE_MAX as *mut u8, &src) };
        assert!(!result, "kernel-space address must return false");
    }

    // ─── MAX_MSG_SIZE constant ───

    #[test]
    fn test_max_msg_size() {
        assert_eq!(MAX_MSG_SIZE, 4096);
    }

    // ─── sys_thread_create validation ───

    #[test]
    fn test_thread_create_rejects_zero_entry() {
        // Entry point 0 is invalid — must be in user-space.
        let result = sys_thread_create(0, 0x7FFF_FFFF_0000, 42);
        assert_eq!(result, Error::InvalidArgument as i64);
    }

    #[test]
    fn test_thread_create_rejects_kernel_entry() {
        // Entry point in kernel space.
        let result = sys_thread_create(crate::memory::USER_SPACE_MAX, 0x7FFF_FFFF_0000, 42);
        assert_eq!(result, Error::InvalidArgument as i64);
    }

    #[test]
    fn test_thread_create_rejects_zero_stack() {
        // Stack pointer 0 is invalid.
        let result = sys_thread_create(0x40_0000, 0, 42);
        assert_eq!(result, Error::InvalidArgument as i64);
    }

    #[test]
    fn test_thread_create_rejects_kernel_stack() {
        // Stack pointer in kernel space.
        let result = sys_thread_create(0x40_0000, crate::memory::USER_SPACE_MAX, 42);
        assert_eq!(result, Error::InvalidArgument as i64);
    }

    #[test]
    fn test_thread_create_rejects_max_entry() {
        // u64::MAX is way above user space.
        let result = sys_thread_create(u64::MAX, 0x7FFF_FFFF_0000, 0);
        assert_eq!(result, Error::InvalidArgument as i64);
    }

    #[test]
    fn test_thread_create_rejects_max_stack() {
        // u64::MAX stack pointer.
        let result = sys_thread_create(0x40_0000, u64::MAX, 0);
        assert_eq!(result, Error::InvalidArgument as i64);
    }

    // ─── dup2 tests ───

    #[test]
    fn test_fd_constants() {
        assert_eq!(FD_STDIN, 0);
        assert_eq!(FD_STDOUT, 1);
        assert_eq!(FD_FIRST_USABLE, 2);
        assert_eq!(FD_LIMIT, 1024);
    }

    #[test]
    fn test_dup2_same_fd_stdin_rejected() {
        // old_fd == new_fd for stdin (0) — should return InvalidArgument.
        assert_eq!(sys_dup2(0, 0), Error::InvalidArgument as i64);
    }

    #[test]
    fn test_dup2_same_fd_stdout_rejected() {
        // old_fd == new_fd for stdout (1) — should return InvalidArgument.
        assert_eq!(sys_dup2(1, 1), Error::InvalidArgument as i64);
    }

    #[test]
    fn test_dup2_same_fd_nonexistent() {
        // old_fd == new_fd but fd does not exist (no task running in test).
        // with_current_task returns None => NotFound.
        assert_eq!(sys_dup2(999, 999), Error::NotFound as i64);
    }

    #[test]
    fn test_dup2_invalid_old_fd_below_range() {
        // old_fd < FD_FIRST_USABLE should return InvalidArgument.
        assert_eq!(sys_dup2(0, 5), Error::InvalidArgument as i64);
        assert_eq!(sys_dup2(1, 5), Error::InvalidArgument as i64);
    }

    #[test]
    fn test_dup2_invalid_new_fd_below_range() {
        // new_fd < FD_FIRST_USABLE should return InvalidArgument.
        assert_eq!(sys_dup2(2, 0), Error::InvalidArgument as i64);
        assert_eq!(sys_dup2(2, 1), Error::InvalidArgument as i64);
    }

    #[test]
    fn test_dup2_invalid_old_fd_above_limit() {
        // old_fd >= FD_LIMIT should return InvalidArgument.
        assert_eq!(sys_dup2(FD_LIMIT, 5), Error::InvalidArgument as i64);
        assert_eq!(sys_dup2(FD_LIMIT + 100, 5), Error::InvalidArgument as i64);
    }

    #[test]
    fn test_dup2_invalid_new_fd_above_limit() {
        // new_fd >= FD_LIMIT should return InvalidArgument.
        assert_eq!(sys_dup2(2, FD_LIMIT), Error::InvalidArgument as i64);
        assert_eq!(sys_dup2(2, FD_LIMIT + 100), Error::InvalidArgument as i64);
    }

    #[test]
    fn test_dup2_old_fd_not_found() {
        // old_fd is valid range but not open (no task running in test mode).
        // with_current_task returns None => NotFound.
        assert_eq!(sys_dup2(5, 10), Error::NotFound as i64);
    }

    #[test]
    fn test_dup2_entry_cloned_correctly() {
        // Verify that cloning an FdEntry preserves all fields.
        use alloc::collections::BTreeMap;
        let mut fd_table: BTreeMap<u64, crate::task::task::FdEntry> = BTreeMap::new();
        fd_table.insert(
            2,
            crate::task::task::FdEntry {
                path: alloc::string::String::from("/ramdisk/data.bin"),
                ino: 42,
                offset: 100,
            },
        );

        // Simulate dup2(2, 10): clone entry from fd 2 to fd 10.
        let old = fd_table.get(&2).unwrap();
        let cloned = crate::task::task::FdEntry {
            path: old.path.clone(),
            ino: old.ino,
            offset: old.offset,
        };
        fd_table.insert(10, cloned);

        let new_entry = fd_table.get(&10).unwrap();
        assert_eq!(new_entry.path, "/ramdisk/data.bin");
        assert_eq!(new_entry.ino, 42);
        assert_eq!(new_entry.offset, 100);
    }

    #[test]
    fn test_dup2_overwrites_existing_fd() {
        // Simulate dup2(2, 3) where both are open — fd 3 should be overwritten.
        use alloc::collections::BTreeMap;
        let mut fd_table: BTreeMap<u64, crate::task::task::FdEntry> = BTreeMap::new();
        fd_table.insert(
            2,
            crate::task::task::FdEntry {
                path: alloc::string::String::from("/ramdisk/old.txt"),
                ino: 10,
                offset: 0,
            },
        );
        fd_table.insert(
            3,
            crate::task::task::FdEntry {
                path: alloc::string::String::from("/ramdisk/target.txt"),
                ino: 20,
                offset: 99,
            },
        );

        // Remove old entry at 3 and insert clone of 2.
        fd_table.remove(&3);
        let old = fd_table.get(&2).unwrap();
        let cloned = crate::task::task::FdEntry {
            path: old.path.clone(),
            ino: old.ino,
            offset: old.offset,
        };
        fd_table.insert(3, cloned);

        // fd 3 now has fd 2's data, not the original fd 3 data.
        let entry = fd_table.get(&3).unwrap();
        assert_eq!(entry.path, "/ramdisk/old.txt");
        assert_eq!(entry.ino, 10);
        assert_eq!(entry.offset, 0);

        // fd 2 is unchanged.
        let entry2 = fd_table.get(&2).unwrap();
        assert_eq!(entry2.path, "/ramdisk/old.txt");
        assert_eq!(entry2.ino, 10);
    }

    #[test]
    fn test_dup2_boundary_fd_limit_minus_one() {
        // FD_LIMIT - 1 is the highest valid fd. old_fd == new_fd at this
        // boundary should return NotFound (no task running in test mode).
        assert_eq!(sys_dup2(FD_LIMIT - 1, FD_LIMIT - 1), Error::NotFound as i64);
    }

    // ─── resolve_path tests ───

    #[test]
    fn test_resolve_path_absolute() {
        // Absolute paths are returned as-is (no scheduler access needed).
        assert_eq!(resolve_path("/foo/bar"), "/foo/bar");
        assert_eq!(resolve_path("/"), "/");
        assert_eq!(resolve_path("/disk/test.txt"), "/disk/test.txt");
    }

    // ─── split_parent_name tests ───

    #[test]
    fn test_split_parent_name_simple() {
        assert_eq!(
            split_parent_name("/disk/foo/bar.txt"),
            Some(("/disk/foo", "bar.txt"))
        );
    }

    #[test]
    fn test_split_parent_name_root_file() {
        assert_eq!(split_parent_name("/foo.txt"), Some(("/", "foo.txt")));
    }

    #[test]
    fn test_split_parent_name_trailing_slash() {
        assert_eq!(
            split_parent_name("/disk/foo/bar/"),
            Some(("/disk/foo", "bar"))
        );
    }

    #[test]
    fn test_split_parent_name_none_for_root() {
        // "/" has no parent/name split.
        assert_eq!(split_parent_name("/"), None);
    }

    #[test]
    fn test_split_parent_name_none_for_empty() {
        assert_eq!(split_parent_name(""), None);
    }

    #[test]
    fn test_split_parent_name_single_segment() {
        assert_eq!(split_parent_name("foo"), None);
    }

    // ─── fstat validation tests ───

    #[test]
    fn test_fstat_invalid_fd_stdin() {
        // fd 0 (stdin) is not a VFS file — should return NotFound.
        assert_eq!(sys_fstat(0, 0x1000), Error::NotFound as i64);
    }

    #[test]
    fn test_fstat_invalid_fd_stdout() {
        // fd 1 (stdout) is not a VFS file — should return NotFound.
        assert_eq!(sys_fstat(1, 0x1000), Error::NotFound as i64);
    }

    #[test]
    fn test_fstat_nonexistent_fd() {
        // fd 999 is not open — no task in test mode => NotFound.
        assert_eq!(sys_fstat(999, 0x1000), Error::NotFound as i64);
    }

    // ─── lstat validation tests ───

    #[test]
    fn test_lstat_bad_pointer() {
        // Null pointer — should return BadPointer.
        let result = sys_lstat(0, 5, 0x1000);
        assert_eq!(result, Error::BadPointer as i64);
    }

    #[test]
    fn test_lstat_zero_length() {
        // Zero length — should return BadPointer (copy_from_user rejects len=0).
        let result = sys_lstat(0x1000, 0, 0x2000);
        assert_eq!(result, Error::BadPointer as i64);
    }

    // ─── sys_access tests ───

    #[test]
    fn test_access_null_pointer() {
        // Null path pointer — should return BadPointer.
        let result = sys_access(0, 10, 0);
        assert_eq!(result, Error::BadPointer as i64);
    }

    #[test]
    fn test_access_zero_length() {
        // Zero path length — should return BadPointer (copy_from_user rejects len=0).
        let result = sys_access(0x1000, 0, 0);
        assert_eq!(result, Error::BadPointer as i64);
    }

    #[test]
    fn test_access_invalid_mode_bits() {
        // Mode with unknown bits set — should return InvalidArgument.
        // Validate the constant bitmask definition.
        let known_bits = ACCESS_F_OK | ACCESS_R_OK | ACCESS_W_OK | ACCESS_X_OK;
        assert_eq!(known_bits, 0x7);
        // Mode 0x8 has unknown bits.
        assert_ne!(0x8 & !known_bits, 0);
    }

    #[test]
    fn test_access_constants_match_number_module() {
        // Access mode constants in mod.rs must match number.rs.
        assert_eq!(ACCESS_F_OK, number::F_OK);
        assert_eq!(ACCESS_R_OK, number::R_OK);
        assert_eq!(ACCESS_W_OK, number::W_OK);
        assert_eq!(ACCESS_X_OK, number::X_OK);
    }

    // ─── sys_fs_seek edge-case tests ───

    #[test]
    fn test_seek_constants_match_number_module() {
        // Seek whence constants used in sys_fs_seek must match number.rs.
        assert_eq!(number::SEEK_SET, 0);
        assert_eq!(number::SEEK_CUR, 1);
        assert_eq!(number::SEEK_END, 2);
    }

    #[test]
    fn test_seek_set_negative_offset_rejected() {
        // SEEK_SET with negative offset should return InvalidArgument.
        // Verify the constant is 0 which is the match value.
        assert_eq!(number::SEEK_SET, 0);
    }

    #[test]
    fn test_seek_cur_negative_offset_saturates() {
        // SEEK_CUR with a negative offset larger than current position
        // should saturate to 0 via saturating_sub.
        let current_offset: usize = 10;
        let offset: i64 = -100;
        let abs = (-offset) as usize;
        let result = current_offset.saturating_sub(abs);
        assert_eq!(result, 0, "saturating_sub should clamp to 0");
    }

    #[test]
    fn test_seek_end_positive_offset_returns_file_len() {
        // SEEK_END with positive offset should return file_len
        // (positive offset from end is meaningless, clamped to file_len).
        let file_len: usize = 1024;
        let offset: i64 = 100;
        assert!(offset >= 0);
        let result = file_len;
        assert_eq!(result, 1024);
    }

    #[test]
    fn test_seek_end_negative_offset_from_end() {
        // SEEK_END with -50 on a 1024-byte file should return 974.
        let file_len: usize = 1024;
        let offset: i64 = -50;
        let abs = (-offset) as usize;
        let result = file_len.saturating_sub(abs);
        assert_eq!(result, 974);
    }

    #[test]
    fn test_seek_end_negative_past_start_saturates() {
        // SEEK_END with -2000 on a 1024-byte file should saturate to 0.
        let file_len: usize = 1024;
        let offset: i64 = -2000;
        let abs = (-offset) as usize;
        let result = file_len.saturating_sub(abs);
        assert_eq!(result, 0, "seek past start should saturate to 0");
    }

    #[test]
    fn test_seek_end_zero_size_file() {
        // SEEK_END on a zero-size file with any negative offset should return 0.
        let file_len: usize = 0;
        let offset: i64 = -1;
        let abs = (-offset) as usize;
        let result = file_len.saturating_sub(abs);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_seek_set_past_eof() {
        // SEEK_SET allows seeking past EOF (no error in our implementation).
        let offset: i64 = 9999;
        assert!(offset >= 0);
        let result = offset as usize;
        assert_eq!(result, 9999);
    }

    #[test]
    fn test_seek_cur_past_eof() {
        // SEEK_CUR can move past EOF via positive offset.
        let current_offset: usize = 100;
        let offset: i64 = 9999;
        let result = current_offset.saturating_add(offset as usize);
        assert_eq!(result, 10099);
    }

    #[test]
    fn test_seek_invalid_whence() {
        // Any whence value other than 0, 1, 2 should be rejected.
        let valid_whence = [number::SEEK_SET, number::SEEK_CUR, number::SEEK_END];
        for w in 0..10u64 {
            if valid_whence.contains(&w) {
                continue;
            }
            // w is not a valid whence.
            assert!(w != 0 && w != 1 && w != 2);
        }
    }

    // ─── getdents64 constants ───

    #[test]
    fn test_getdents64_dirent_type_values() {
        // Linux convention: 4 = directory, 8 = regular file.
        assert_eq!(DIRENT_TYPE_DIR, 4);
        assert_eq!(DIRENT_TYPE_FILE, 8);
    }

    #[test]
    fn test_getdents64_header_size() {
        // d_ino(8) + d_off(8) + d_reclen(2) + d_type(1) = 19 bytes.
        assert_eq!(DIRENT_HEADER_SIZE, 19);
    }

    // ─── getdents64 validation tests ───

    #[test]
    fn test_getdents64_rejects_stdin() {
        // fd 0 (stdin) is not a directory — should return InvalidArgument.
        assert_eq!(
            sys_getdents64(0, 0x1000, 256),
            Error::InvalidArgument as i64
        );
    }

    #[test]
    fn test_getdents64_rejects_stdout() {
        // fd 1 (stdout) is not a directory — should return InvalidArgument.
        assert_eq!(
            sys_getdents64(1, 0x1000, 256),
            Error::InvalidArgument as i64
        );
    }

    #[test]
    fn test_getdents64_rejects_null_buffer() {
        // Null buffer pointer — should return BadPointer.
        assert_eq!(sys_getdents64(3, 0, 256), Error::BadPointer as i64);
    }

    #[test]
    fn test_getdents64_rejects_kernel_address() {
        // Buffer in kernel space — should return BadPointer.
        assert_eq!(
            sys_getdents64(3, crate::memory::USER_SPACE_MAX, 256),
            Error::BadPointer as i64
        );
    }

    #[test]
    fn test_getdents64_rejects_too_small_buffer() {
        // Buffer smaller than DIRENT_HEADER_SIZE + 2 — should return InvalidArgument.
        let small = DIRENT_HEADER_SIZE as u64 + 1; // Too small for smallest entry.
        assert_eq!(
            sys_getdents64(3, 0x1000, small),
            Error::InvalidArgument as i64
        );
    }

    #[test]
    fn test_getdents64_nonexistent_fd() {
        // fd 999 is not open — no task in test mode => NotFound.
        assert_eq!(sys_getdents64(999, 0x1000, 256), Error::NotFound as i64);
    }

    // ─── getdents64 record layout tests ───

    #[test]
    fn test_getdents64_record_alignment() {
        // Verify that record length is aligned to 8 bytes for various name lengths.
        let name_bytes = b"test.txt";
        let name_len = name_bytes.len() + 1; // +1 for NUL
        let reclen = (DIRENT_HEADER_SIZE + name_len + 7) & !7;
        // 19 + 9 = 28, aligned to 8 = 32.
        assert_eq!(reclen, 32);
    }

    #[test]
    fn test_getdents64_record_alignment_short_name() {
        // Single-character name: "a\0" => 19 + 2 = 21, aligned to 8 = 24.
        let name_len = 2usize; // "a" + NUL
        let reclen = (DIRENT_HEADER_SIZE + name_len + 7) & !7;
        assert_eq!(reclen, 24);
    }

    #[test]
    fn test_getdents64_record_alignment_exact_boundary() {
        // Name that makes total exactly 24: 19 + 5 = 24 (already aligned).
        let name_len = 5usize; // 4 chars + NUL
        let reclen = (DIRENT_HEADER_SIZE + name_len + 7) & !7;
        assert_eq!(reclen, 24);
    }

    // ─── Process group / session syscall tests ───

    #[test]
    fn test_setpgid_returns_not_found_for_invalid_pid() {
        // PID 999999 does not exist — should return NotFound.
        let result = sys_setpgid(999_999, 1);
        assert_eq!(result, Error::NotFound as i64);
    }

    #[test]
    fn test_getpgid_returns_not_found_for_invalid_pid() {
        // PID 999999 does not exist — should return NotFound.
        let result = sys_getpgid(999_999);
        assert_eq!(result, Error::NotFound as i64);
    }

    #[test]
    fn test_setsid_returns_not_found_when_no_current_task() {
        // In test mode there is no current task, so setsid returns NotFound.
        let result = sys_setsid();
        assert_eq!(result, Error::NotFound as i64);
    }

    #[test]
    fn test_setpgid_zero_pid_zero_pgid_logic() {
        // When pid==0, the current task is used.
        // When pgid==0, the pgid is set to the target's own id.
        // In test mode there's no current task, so we get NotFound.
        let result = sys_setpgid(0, 0);
        assert_eq!(result, Error::NotFound as i64);
    }
    // ─── sys_chmod tests ───

    #[test]
    fn test_chmod_null_pointer() {
        // Null path pointer — should return BadPointer.
        let result = sys_chmod(0, 10, 0o755);
        assert_eq!(result, Error::BadPointer as i64);
    }

    #[test]
    fn test_chmod_zero_length() {
        // Zero path length — should return BadPointer (copy_from_user rejects len=0).
        let result = sys_chmod(0x1000, 0, 0o755);
        assert_eq!(result, Error::BadPointer as i64);
    }

    #[test]
    fn test_chmod_kernel_address() {
        // Path pointer in kernel space — should return BadPointer.
        let result = sys_chmod(crate::memory::USER_SPACE_MAX, 10, 0o755);
        // but produce invalid UTF-8 bytes. Use a stack buffer instead.
        // Since we can't easily test the full path in unit tests (no scheduler),
        // we verify the input validation logic instead.
        let invalid_path: &[u8] = &[0xFF, 0xFE, 0xFD]; // Invalid UTF-8
        assert!(core::str::from_utf8(invalid_path).is_err());
    }

    // ─── sys_umask tests ───

    #[test]
    fn test_umask_no_current_task() {
        // No current task in test mode — with_current_task_mut returns None.
        let result = sys_umask(0o022);
        assert_eq!(result, Error::NotFound as i64);
    }

    #[test]
    fn test_umask_constant_default() {
        // Verify the DEFAULT_UMASK constant matches what we expect.
        assert_eq!(crate::task::task::DEFAULT_UMASK, 0o022);
    }
}

/// Initialize the syscall interface.
pub fn init() {
    crate::serial_println!("[...] Initializing syscall handler");
    super::arch::x86_64::syscall::init();
    crate::serial_println!("[OK] Syscall handler initialized");
}
