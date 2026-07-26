//! System call interface.
//!
//! User-space invokes a syscall via the `syscall` instruction. The assembly
//! stub in `arch/x86_64/syscall.rs` saves registers, calls `handle_syscall_raw`,
//! and either SYSRETs back to user-space or switches to a different task.
//!
//! Error convention (INTERFACE.md §3.1):
//!   - Positive return = success value
//!   - Negative return = error code

pub mod number;

use number::{
    SYS_CHANNEL_CALL, SYS_CHANNEL_CREATE, SYS_CHANNEL_RECEIVE, SYS_CHANNEL_REPLY, SYS_CHANNEL_SEND,
    SYS_CONSOLE_READ, SYS_CONSOLE_WRITE, SYS_ENDPOINT_DISCOVER, SYS_ENDPOINT_REGISTER,
    SYS_EVENT_CREATE, SYS_EVENT_DESTROY, SYS_EVENT_SIGNAL, SYS_EVENT_WAIT, SYS_FS_CLOSE,
    SYS_FS_OPEN, SYS_FS_READ, SYS_FS_WRITE, SYS_HANDLE_CLOSE, SYS_HANDLE_DUPLICATE,
    SYS_HANDLE_TRANSFER, SYS_PROCESS_CREATE, SYS_PROCESS_EXIT, SYS_PROCESS_START, SYS_PROCESS_WAIT,
    SYS_SLEEP, SYS_THREAD_CREATE, SYS_THREAD_EXIT, SYS_THREAD_YIELD,
};

use crate::handle::{Handle, KernelObject, Rights};
use crate::ipc::{Channel, EndId};

/// Error codes (INTERFACE.md §3.1). Negative values returned in RAX.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum Error {
    InvalidArgument = -1,
    NotFound = -2,
    PermissionDenied = -3,
    OutOfMemory = -4,
    Busy = -5,
    ChannelClosed = -6,
    WouldBlock = -7,
    Timeout = -8,
    BadPointer = -9,
    UnknownSyscall = -10,
}

/// Maximum message size (4 KiB).
const MAX_MSG_SIZE: usize = 4096;

/// Copy bytes from user-space into a kernel buffer.
unsafe fn copy_from_user(src: *const u8, len: usize) -> Option<alloc::vec::Vec<u8>> {
    if src.is_null() || len == 0 || len > MAX_MSG_SIZE {
        return None;
    }
    if (src as u64) >= crate::memory::USER_SPACE_MAX {
        return None;
    }
    let mut buf = alloc::vec![0u8; len];
    unsafe { core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), len) }
    Some(buf)
}

/// Copy bytes from a kernel buffer into user-space.
unsafe fn copy_to_user(dst: *mut u8, src: &[u8]) -> bool {
    if dst.is_null() || src.is_empty() || src.len() > MAX_MSG_SIZE {
        return false;
    }
    if (dst as u64) >= crate::memory::USER_SPACE_MAX {
        return false;
    }
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
        SYS_FS_CLOSE => sys_fs_close(arg1),

        _ => Error::UnknownSyscall as i64,
    }
}

// ─────────────────── Channel syscalls ───────────────────

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
    }
}

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
            crate::ipc::RecvResult::GotMessage(msg) => {
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
    crate::serial_println!("[SYSCALL] channel_receive: no other task, spin-wait");
    loop {
        x86_64::instructions::hlt();
        let mut ch = channel.lock();
        match ch.receive(end, task_id) {
            crate::ipc::RecvResult::GotMessage(msg) => {
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
        }
    }
}

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
        crate::ipc::CallResult::Blocked => {
            crate::serial_println!("[SYSCALL] channel_call: blocked, waiting for reply");
            drop(ch);
            // Spin-wait for the reply (the server will call channel_reply).
            loop {
                let mut ch = channel.lock();
                if let crate::ipc::CallResult::GotReply(reply) =
                    ch.call(end, alloc::vec![], task_id)
                {
                    let len = reply.len();
                    drop(ch);
                    if unsafe { copy_to_user(reply_ptr as *mut u8, &reply) } {
                        return i64::try_from(len).unwrap_or(-1);
                    }
                    return Error::BadPointer as i64;
                }
                drop(ch);
                x86_64::instructions::hlt();
            }
        }
    }
}

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
    ch.pending_handles.push(handle_raw);

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

    let task_id = crate::task::scheduler::spawn_task_with_id(name, 0);
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
fn sys_process_start(_proc_raw: u64, name_ptr: u64, name_len: u64) -> i64 {
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
        "[SYSCALL] process_start: loading '{}' ({} bytes)",
        filename,
        file.data.len()
    );

    // Parse the ELF entry point.
    let Ok(header) = crate::elf::parse_header(file.data) else {
        crate::serial_println!("[SYSCALL] process_start: ELF parse error");
        return Error::InvalidArgument as i64;
    };
    let entry = header.entry;

    // Allocate a new page table for the process (kernel entries shared).
    let page_table_phys = crate::memory::create_user_page_table();

    // Create a new task with the entry point, parented to the current task.
    let mut task = crate::task::task::Task::new(filename, 0);
    task.parent_id = Some(crate::task::scheduler::current_task_id());
    task.page_table = page_table_phys;
    let task_id = task.id;

    // Set the task context with the ELF entry point.
    // The stack will be allocated by the ELF loader when the task runs.
    crate::task::scheduler::spawn_task_from(task);

    crate::serial_println!(
        "[SYSCALL] process_start: task {} entry={:#x}",
        task_id.as_u64(),
        entry
    );

    i64::try_from(task_id.as_u64()).unwrap_or(Error::InvalidArgument as i64)
}

fn sys_process_exit(status: u64) -> i64 {
    crate::serial_println!("[SYS_EXIT] status={status}");
    0
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
        let status = crate::task::scheduler::with_current_task(|_task| {
            // Look up the child task's exit_status.
            // For now, check if the child is still in the scheduler.
            // A full implementation would check task.exit_status.
            None::<u64>
        });

        // For now, return after timeout since we don't track child exit status yet.
        let current =
            crate::arch::x86_64::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);
        if current - start >= timeout {
            break;
        }

        x86_64::instructions::hlt();
    }

    crate::serial_println!("[SYSCALL] process_wait: timeout");
    Error::WouldBlock as i64
}

// ─────────────────── Thread syscalls ───────────────────

fn sys_thread_create(_proc_raw: u64) -> i64 {
    crate::serial_println!("[SYSCALL] thread_create");
    0
}

fn sys_thread_exit() -> i64 {
    crate::serial_println!("[SYSCALL] thread_exit");
    loop {
        x86_64::instructions::hlt();
    }
}

fn sys_thread_yield() -> i64 {
    0
}

// ─────────────────── Timer / Sleep ───────────────────

/// Sleep for the specified number of timer ticks (~18.2 Hz per tick).
/// Blocks the calling task by spinning with HLT until enough ticks elapse.
fn sys_sleep(ticks: u64) -> i64 {
    let start = crate::arch::x86_64::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);
    let target = start + ticks;
    crate::serial_println!("[SYSCALL] sleep: {} ticks (from {})", ticks, start);
    loop {
        x86_64::instructions::hlt();
        let current =
            crate::arch::x86_64::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);
        if current >= target {
            break;
        }
    }
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

// ─────────────────── Debug console ───────────────────

fn sys_console_write(ptr: u64, len: u64) -> i64 {
    let Some(msg) = (unsafe { copy_from_user(ptr as *const u8, len as usize) }) else {
        return Error::BadPointer as i64;
    };
    for &byte in &msg {
        if (0x20..=0x7e).contains(&byte) || byte == b'\n' {
            crate::serial_print!("{}", byte as char);
        }
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

    // Cap at MAX_MSG_SIZE to prevent abuse
    let len = buf_len.min(MAX_MSG_SIZE as u64) as usize;

    let blocking = (flags & 1) != 0;

    // SAFETY: We've validated the pointer is non-null and in user-space.
    // The keyboard driver will write at most `len` bytes.
    let bytes_read = crate::drivers::keyboard::read(buf_ptr as *mut u8, len, blocking);

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

// ─────────────────── Filesystem (ramfs) ───────────────────

/// Maximum number of file descriptors.
const MAX_FD: usize = 32;

/// Reserved fd for stdin (not backed by ramfs).
const FD_STDIN: u64 = 0;

/// Reserved fd for stdout (not backed by ramfs).
const FD_STDOUT: u64 = 1;

/// First usable fd for ramfs files.
const FD_FIRST_USABLE: u64 = 2;

/// A file descriptor entry mapping an fd number to a ramfs filename.
struct FdEntry {
    /// Filename in the ramfs.
    name: alloc::string::String,
    /// Current read/write offset within the file.
    offset: usize,
}

/// Global fd table. Fixed-size array protected by a spinlock.
static FD_TABLE: spin::Mutex<[Option<FdEntry>; MAX_FD]> = spin::Mutex::new([
    None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None,
    None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None,
]);

/// Open a file from ramfs and return a file descriptor.
///
/// Arguments:
///   arg0: pointer to filename (UTF-8)
///   arg1: filename length
///   arg2: flags (reserved, must be 0 for now)
///
/// Returns: fd >= 0 on success, negative error code on failure.
fn sys_fs_open(name_ptr: u64, name_len: u64, flags: u64) -> i64 {
    if flags != 0 {
        return Error::InvalidArgument as i64;
    }

    let Some(name_bytes) = (unsafe { copy_from_user(name_ptr as *const u8, name_len as usize) })
    else {
        return Error::BadPointer as i64;
    };
    let Ok(filename) = core::str::from_utf8(&name_bytes) else {
        return Error::InvalidArgument as i64;
    };

    // Verify the file exists in ramfs.
    if crate::fs::ramfs::read_file(filename).is_err() {
        return Error::NotFound as i64;
    }

    let mut table = FD_TABLE.lock();
    // Find the first free slot starting after stdin/stdout.
    for (i, entry) in table.iter_mut().enumerate().skip(FD_FIRST_USABLE as usize) {
        if entry.is_none() {
            *entry = Some(FdEntry {
                name: alloc::string::String::from(filename),
                offset: 0,
            });
            crate::serial_println!("[SYSCALL] fs_open: '{}' -> fd {}", filename, i);
            #[allow(clippy::cast_possible_wrap)]
            {
                return i as i64;
            }
        }
    }

    Error::OutOfMemory as i64
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
    if fd >= MAX_FD as u64 {
        return Error::InvalidArgument as i64;
    }

    let filename;
    let offset;
    {
        let mut table = FD_TABLE.lock();
        let Some(Some(entry)) = table.get_mut(fd as usize) else {
            return Error::NotFound as i64;
        };
        filename = entry.name.clone();
        offset = entry.offset;
    }

    let Ok(data) = crate::fs::ramfs::read_file(&filename) else {
        return Error::NotFound as i64;
    };

    if offset >= data.len() {
        return 0; // EOF
    }

    let available = &data[offset..];
    let to_copy = available.len().min(buf_len as usize);
    if !unsafe { copy_to_user(buf_ptr as *mut u8, &available[..to_copy]) } {
        return Error::BadPointer as i64;
    }

    // Advance the offset.
    {
        let mut table = FD_TABLE.lock();
        if let Some(Some(entry)) = table.get_mut(fd as usize) {
            entry.offset += to_copy;
        }
    }

    crate::serial_println!("[SYSCALL] fs_read: fd {} read {} bytes", fd, to_copy);
    #[allow(clippy::cast_possible_wrap)]
    {
        to_copy as i64
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
            if (0x20..=0x7e).contains(&byte) || byte == b'\n' {
                crate::serial_print!("{}", byte as char);
            }
        }
        return i64::try_from(data_len).unwrap_or(-1);
    }

    if fd == FD_STDIN {
        return Error::InvalidArgument as i64;
    }
    if fd >= MAX_FD as u64 {
        return Error::InvalidArgument as i64;
    }

    let Some(data) = (unsafe { copy_from_user(data_ptr as *const u8, data_len as usize) }) else {
        return Error::BadPointer as i64;
    };

    let filename;
    {
        let table = FD_TABLE.lock();
        let Some(Some(entry)) = table.get(fd as usize) else {
            return Error::NotFound as i64;
        };
        filename = entry.name.clone();
    }

    if crate::fs::ramfs::write_file(&filename, &data).is_err() {
        return Error::InvalidArgument as i64;
    }

    crate::serial_println!("[SYSCALL] fs_write: fd {} wrote {} bytes", fd, data.len());
    i64::try_from(data.len()).unwrap_or(-1)
}

/// Close a file descriptor.
///
/// Arguments:
///   arg0: file descriptor
///
/// Returns: 0 on success, negative error code on failure.
fn sys_fs_close(fd: u64) -> i64 {
    if fd < FD_FIRST_USABLE || fd >= MAX_FD as u64 {
        return Error::InvalidArgument as i64;
    }

    let mut table = FD_TABLE.lock();
    if table[fd as usize].is_some() {
        table[fd as usize] = None;
        crate::serial_println!("[SYSCALL] fs_close: fd {}", fd);
        0
    } else {
        Error::NotFound as i64
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
            assert!(*err as i64 < 0, "{:?} must be negative", err);
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
