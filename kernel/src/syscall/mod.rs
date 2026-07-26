//! System call interface.
//!
//! User-space invokes a syscall via the `syscall` instruction. The assembly
//! stub in `arch/x86_64/syscall.rs` saves registers, calls `handle_syscall_raw`,
//! and SYSRETs back to user-space.
//!
//! Error convention (INTERFACE.md §3.1):
//!   - Positive return = success value
//!   - Negative return = error code

pub mod number;

use number::{
    SYS_CHANNEL_CALL, SYS_CHANNEL_CREATE, SYS_CHANNEL_RECEIVE, SYS_CHANNEL_REPLY, SYS_CHANNEL_SEND,
    SYS_CONSOLE_WRITE, SYS_HANDLE_CLOSE, SYS_HANDLE_DUPLICATE, SYS_HANDLE_TRANSFER,
    SYS_PROCESS_CREATE, SYS_PROCESS_EXIT, SYS_PROCESS_START, SYS_PROCESS_WAIT, SYS_THREAD_CREATE,
    SYS_THREAD_EXIT, SYS_THREAD_YIELD,
};

use crate::handle::{Handle, KernelObject, Rights};
use crate::ipc::{self, CallResult, EndId, RecvResult, ReplyResult, SendResult};

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
///
/// # Safety
/// `src` must be a valid user-space pointer with at least `len` readable bytes.
unsafe fn copy_from_user(src: *const u8, len: usize) -> Option<alloc::vec::Vec<u8>> {
    if src.is_null() || len == 0 || len > MAX_MSG_SIZE {
        return None;
    }
    let src_addr = src as u64;
    if src_addr >= crate::memory::USER_SPACE_MAX {
        return None;
    }
    let mut buf = alloc::vec![0u8; len];
    unsafe {
        core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), len);
    }
    Some(buf)
}

/// Copy bytes from a kernel buffer into user-space.
///
/// # Safety
/// `dst` must be a valid user-space pointer with at least `src.len()` writable bytes.
unsafe fn copy_to_user(dst: *mut u8, src: &[u8]) -> bool {
    let len = src.len();
    if dst.is_null() || len == 0 || len > MAX_MSG_SIZE {
        return false;
    }
    let dst_addr = dst as u64;
    if dst_addr >= crate::memory::USER_SPACE_MAX {
        return false;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(src.as_ptr(), dst, len);
    }
    true
}

/// Raw syscall handler called from the assembly stub.
///
/// Arguments: RDI=number, RSI=arg1, RDX=arg2, RCX=arg3, R8=arg4, R9=arg5
/// Returns: positive = success, negative = error code.
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
        SYS_PROCESS_START => sys_process_start(arg1, arg2, arg3, arg4, arg5),
        SYS_PROCESS_EXIT => sys_process_exit(arg1),
        SYS_PROCESS_WAIT => sys_process_wait(arg1, arg2),

        SYS_THREAD_CREATE => sys_thread_create(arg1),
        SYS_THREAD_EXIT => sys_thread_exit(),
        SYS_THREAD_YIELD => sys_thread_yield(),

        SYS_CONSOLE_WRITE => sys_console_write(arg1, arg2),

        _ => Error::UnknownSyscall as i64,
    }
}

/// Get the current task's ID for blocking/waking operations.
fn current_task_id() -> u64 {
    crate::task::scheduler::current_task_id().as_u64()
}

// ─────────────────── Channel syscalls ───────────────────

/// Create a new channel. Returns two handle values in a packed u64:
/// low 32 bits = `handle_a`, high 32 bits = `handle_b`.
fn sys_channel_create() -> i64 {
    let (channel_id, end_a, end_b) = ipc::create_channel();

    // Insert both ends into the current task's handle table.
    // For now, return the channel_id as both handles (simplified).
    // In a full implementation, each end would be a separate Handle entry.
    let handle_a = Handle::new(0, Rights::ALL, 0).as_u64(); // placeholder
    let handle_b = Handle::new(1, Rights::ALL, 0).as_u64(); // placeholder

    crate::serial_println!("[SYSCALL] channel_create: id={channel_id}");
    // Pack both handles: low=handle_a, high=handle_b
    // For now, return the channel_id (user-space uses it directly)
    i64::try_from(channel_id).unwrap_or(Error::InvalidArgument as i64)
}

/// Send a message on a channel.
fn sys_channel_send(handle_raw: u64, msg_ptr: u64, msg_len: u64) -> i64 {
    let Some(msg) = (unsafe { copy_from_user(msg_ptr as *const u8, msg_len as usize) }) else {
        return Error::BadPointer as i64;
    };

    // For Phase 2: handle_raw is the channel_id (simplified).
    let channel_id = handle_raw;
    let task_id = current_task_id();

    match ipc::channel_send(channel_id, EndId::A, msg, task_id) {
        SendResult::Delivered(receiver_id) => {
            crate::serial_println!("[SYSCALL] channel_send: delivered to task {receiver_id}");
            // Wake the receiver.
            let rid = crate::task::task::TaskId::from_u64(receiver_id);
            crate::task::scheduler::wake_task_by_id(rid);
            0
        }
        SendResult::Pending => {
            crate::serial_println!("[SYSCALL] channel_send: pending, blocking task {task_id}");
            // Block until receiver calls receive.
            crate::task::scheduler::block_current_task();
            0
        }
    }
}

/// Receive a message on a channel.
fn sys_channel_receive(handle_raw: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    let channel_id = handle_raw;
    let task_id = current_task_id();

    match ipc::channel_receive(channel_id, EndId::B, task_id) {
        RecvResult::GotMessage(msg) => {
            crate::serial_println!("[SYSCALL] channel_receive: got {} bytes", msg.len());
            let len = msg.len();
            if unsafe { copy_to_user(buf_ptr as *mut u8, &msg) } {
                i64::try_from(len).unwrap_or(-1)
            } else {
                Error::BadPointer as i64
            }
        }
        RecvResult::Blocked => {
            crate::serial_println!("[SYSCALL] channel_receive: blocked, task {task_id}");
            crate::task::scheduler::block_current_task();
            Error::WouldBlock as i64
        }
    }
}

/// Call = send + block for reply (atomic RPC).
fn sys_channel_call(
    handle_raw: u64,
    msg_ptr: u64,
    msg_len: u64,
    reply_ptr: u64,
    reply_len: u64,
) -> i64 {
    let Some(msg) = (unsafe { copy_from_user(msg_ptr as *const u8, msg_len as usize) }) else {
        return Error::BadPointer as i64;
    };

    let channel_id = handle_raw;
    let task_id = current_task_id();

    match ipc::channel_call(channel_id, EndId::A, msg, task_id) {
        CallResult::GotReply(reply) => {
            crate::serial_println!("[SYSCALL] channel_call: got reply ({} bytes)", reply.len());
            let len = reply.len();
            if unsafe { copy_to_user(reply_ptr as *mut u8, &reply) } {
                i64::try_from(len).unwrap_or(-1)
            } else {
                Error::BadPointer as i64
            }
        }
        CallResult::Blocked => {
            crate::serial_println!("[SYSCALL] channel_call: blocked, task {task_id}");
            crate::task::scheduler::block_current_task();
            Error::WouldBlock as i64
        }
    }
}

/// Reply to a received message. Unblocks the caller's `call`.
fn sys_channel_reply(handle_raw: u64, msg_ptr: u64, msg_len: u64) -> i64 {
    let Some(msg) = (unsafe { copy_from_user(msg_ptr as *const u8, msg_len as usize) }) else {
        return Error::BadPointer as i64;
    };

    let channel_id = handle_raw;

    match ipc::channel_reply(channel_id, EndId::B, msg) {
        ReplyResult::Unblocked(caller_id) => {
            crate::serial_println!("[SYSCALL] channel_reply: unblocked caller {caller_id}");
            let cid = crate::task::task::TaskId::from_u64(caller_id);
            crate::task::scheduler::wake_task_by_id(cid);
            0
        }
        ReplyResult::Stored => {
            crate::serial_println!("[SYSCALL] channel_reply: stored (no caller waiting)");
            0
        }
    }
}

// ─────────────────── Handle syscalls ───────────────────

fn sys_handle_close(handle_raw: u64) -> i64 {
    crate::serial_println!("[SYSCALL] handle_close: {handle_raw:#x}");
    // TODO: remove from task's handle table
    0
}

fn sys_handle_duplicate(handle_raw: u64, new_rights: u64) -> i64 {
    crate::serial_println!("[SYSCALL] handle_duplicate: {handle_raw:#x} rights={new_rights:#x}");
    // TODO: duplicate handle with narrowed rights
    0
}

fn sys_handle_transfer(handle_raw: u64, channel_raw: u64, rights: u64) -> i64 {
    crate::serial_println!(
        "[SYSCALL] handle_transfer: handle={handle_raw:#x} channel={channel_raw:#x} rights={rights:#x}"
    );
    // TODO: transfer handle through channel
    0
}

// ─────────────────── Process syscalls ───────────────────

fn sys_process_create(_job_raw: u64, name_ptr: u64, name_len: u64) -> i64 {
    let _name = unsafe { copy_from_user(name_ptr as *const u8, name_len as usize) };
    crate::serial_println!("[SYSCALL] process_create");
    // TODO: create process, return (proc_handle, vmar_handle)
    0
}

fn sys_process_start(_proc_raw: u64, _thread_raw: u64, _entry: u64, _stack: u64, _arg: u64) -> i64 {
    crate::serial_println!("[SYSCALL] process_start");
    // TODO: start thread in process
    0
}

fn sys_process_exit(status: u64) -> i64 {
    crate::serial_println!("[SYS_EXIT] status={status}");
    loop {
        x86_64::instructions::hlt();
    }
}

fn sys_process_wait(_proc_raw: u64, _timeout: u64) -> i64 {
    crate::serial_println!("[SYSCALL] process_wait");
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

/// Initialize the syscall interface.
pub fn init() {
    crate::println!("[...] Initializing syscall handler");
    crate::serial_println!("[...] Initializing syscall handler");
    super::arch::x86_64::syscall::init();
    crate::println!("[OK] Syscall handler initialized");
    crate::serial_println!("[OK] Syscall handler initialized");
}
