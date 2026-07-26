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
//! | `0x30-0x3F` | Process         | 4     |
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
    SYS_ACCEPT, SYS_BIND, SYS_CHANNEL_CALL, SYS_CHANNEL_CREATE, SYS_CHANNEL_RECEIVE,
    SYS_CHANNEL_REPLY, SYS_CHANNEL_SEND, SYS_CLOSE_SOCK, SYS_CONNECT, SYS_CONSOLE_READ,
    SYS_CONSOLE_WRITE, SYS_DNS_RESOLVE, SYS_ENDPOINT_DISCOVER, SYS_ENDPOINT_REGISTER,
    SYS_EVENT_CREATE, SYS_EVENT_DESTROY, SYS_EVENT_SIGNAL, SYS_EVENT_WAIT, SYS_FS_CLOSE,
    SYS_FS_OPEN, SYS_FS_READ, SYS_FS_SEEK, SYS_FS_WRITE, SYS_HANDLE_CLOSE, SYS_HANDLE_DUPLICATE,
    SYS_HANDLE_TRANSFER, SYS_IRQ_WAIT, SYS_LISTEN, SYS_MMIO_MAP, SYS_MMIO_UNMAP, SYS_NET_RECEIVE,
    SYS_NET_SEND, SYS_PORT_IN, SYS_PORT_OUT, SYS_PROCESS_CREATE, SYS_PROCESS_EXIT,
    SYS_PROCESS_START, SYS_PROCESS_WAIT, SYS_RECVFROM, SYS_SENDTO, SYS_SLEEP, SYS_SOCKET,
    SYS_THREAD_CREATE, SYS_THREAD_EXIT, SYS_THREAD_YIELD,
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

        SYS_THREAD_CREATE => sys_thread_create(arg1),
        SYS_THREAD_EXIT => sys_thread_exit(),
        SYS_THREAD_YIELD => sys_thread_yield(),

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
fn sys_process_start(proc_raw: u64, name_ptr: u64, name_len: u64) -> i64 {
    let task_id = crate::task::task::TaskId::from_u64(proc_raw);

    let Some(name_bytes) = (unsafe { copy_from_user(name_ptr as *const u8, name_len as usize) })
    else {
        return Error::BadPointer as i64;
    };
    let Ok(filename) = core::str::from_utf8(&name_bytes) else {
        return Error::InvalidArgument as i64;
    };

    // Get the ramdisk from the global set during boot.
    // SAFETY: RAMDISK_DATA is set once during boot and is read-only thereafter.
    let Some(ramdisk) = (unsafe { crate::RAMDISK_DATA }) else {
        return Error::NotFound as i64;
    };

    // Find the ELF file in the initrd.
    let Some(file) = crate::initrd::find_file(ramdisk, filename) else {
        return Error::NotFound as i64;
    };

    crate::serial_println!(
        "[SYSCALL] process_start: loading '{}' ({} bytes) into task {}",
        filename,
        file.data.len(),
        task_id.as_u64()
    );

    // Validate the ELF header before loading.
    if crate::elf::parse_header(file.data).is_err() {
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

    let load_result = crate::elf::load_elf(file.data, |virt, phys, writable, executable| {
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
    let ctx = crate::task::task::SavedContext::user_mode(user_rip, user_rsp);
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

// ─────────────────── Thread syscalls ───────────────────

/// Create a new thread within a process. Currently a stub (not implemented).
///
/// Arguments:
///   arg0: process task ID (reserved)
///
/// Returns: 0 (stub).
fn sys_thread_create(_proc_raw: u64) -> i64 {
    crate::serial_println!("[SYSCALL] thread_create");
    0
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

    // Resolve the path to a filesystem and relative path.
    let (fs, rel_path) = crate::fs::vfs::resolve_fs(filename);

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
                path: alloc::string::String::from(filename),
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
        0 => {
            if offset < 0 {
                return Error::InvalidArgument as i64;
            }
            offset as usize
        }
        // SEEK_CUR: offset from current position.
        1 => {
            if offset >= 0 {
                current_offset.saturating_add(offset as usize)
            } else {
                let abs = (-offset) as usize;
                current_offset.saturating_sub(abs)
            }
        }
        // SEEK_END: offset from end of file.
        2 => {
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
/// Not yet implemented (passive open).
fn sys_listen(_sock_fd: u64) -> i64 {
    crate::serial_println!("[SYSCALL] listen: not implemented");
    Error::InvalidArgument as i64
}

/// Accept an incoming connection on a listening socket (TCP only).
///
/// Arguments:
///   arg0: socket descriptor
///
/// Not yet implemented (passive open).
fn sys_accept(_sock_fd: u64) -> i64 {
    crate::serial_println!("[SYSCALL] accept: not implemented");
    Error::InvalidArgument as i64
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

    if socket_type != crate::net::socket::SocketType::Tcp {
        crate::serial_println!("[SYSCALL] sendto: only TCP supported");
        return Error::InvalidArgument as i64;
    }

    if state != crate::net::socket::SocketState::Connected {
        crate::serial_println!("[SYSCALL] sendto: socket not connected");
        return Error::InvalidArgument as i64;
    }

    // Resolve destination: use provided addr/port if non-zero, otherwise
    // use the connected peer's address.
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
            crate::serial_println!("[SYSCALL] sendto: fd={} sent {} bytes", sock_fd, sent);
            i64::try_from(sent).unwrap_or(-1)
        }
        Err(()) => Error::InvalidArgument as i64,
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
}

/// Initialize the syscall interface.
pub fn init() {
    crate::println!("[...] Initializing syscall handler");
    crate::serial_println!("[...] Initializing syscall handler");
    super::arch::x86_64::syscall::init();
    crate::println!("[OK] Syscall handler initialized");
    crate::serial_println!("[OK] Syscall handler initialized");
}
