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
    SYS_CONSOLE_WRITE, SYS_HANDLE_CLOSE, SYS_HANDLE_DUPLICATE, SYS_HANDLE_TRANSFER,
    SYS_PROCESS_CREATE, SYS_PROCESS_EXIT, SYS_PROCESS_START, SYS_PROCESS_WAIT, SYS_THREAD_CREATE,
    SYS_THREAD_EXIT, SYS_THREAD_YIELD,
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
        None => None,
    })?
}

/// Block the current task and switch to the next ready task.
/// Reads the saved context from the assembly stub's `CURRENT_CONTEXT` global.
fn block_and_switch() {
    unsafe {
        let ctx_ptr = crate::arch::x86_64::syscall::CURRENT_CONTEXT;
        if !ctx_ptr.is_null() {
            let ctx = *ctx_ptr;
            crate::task::scheduler::block_and_switch(ctx);
        }
    }
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
            crate::serial_println!("[SYSCALL] channel_send: pending, blocking");
            drop(ch);
            block_and_switch();
            0
        }
    }
}

fn sys_channel_receive(handle_raw: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    let Some((channel, end)) = lookup_channel(handle_raw) else {
        return Error::NotFound as i64;
    };

    let task_id = crate::task::scheduler::current_task_id().as_u64();
    let mut ch = channel.lock();

    match ch.receive(end, task_id) {
        crate::ipc::RecvResult::GotMessage(msg) => {
            crate::serial_println!("[SYSCALL] channel_receive: got {} bytes", msg.len());
            let len = msg.len();
            drop(ch);
            if unsafe { copy_to_user(buf_ptr as *mut u8, &msg) } {
                i64::try_from(len).unwrap_or(-1)
            } else {
                Error::BadPointer as i64
            }
        }
        crate::ipc::RecvResult::Blocked => {
            crate::serial_println!("[SYSCALL] channel_receive: blocked");
            drop(ch);
            block_and_switch();
            Error::WouldBlock as i64
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
            crate::serial_println!("[SYSCALL] channel_call: blocked");
            drop(ch);
            block_and_switch();
            Error::WouldBlock as i64
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

fn sys_handle_transfer(handle_raw: u64, channel_raw: u64, rights: u64) -> i64 {
    crate::serial_println!(
        "[SYSCALL] handle_transfer: handle={handle_raw:#x} channel={channel_raw:#x} rights={rights:#x}"
    );
    // TODO: remove handle from sender, attach to channel message
    0
}

// ─────────────────── Process syscalls ───────────────────

fn sys_process_create(_job_raw: u64, name_ptr: u64, name_len: u64) -> i64 {
    let _name = unsafe { copy_from_user(name_ptr as *const u8, name_len as usize) };
    crate::serial_println!("[SYSCALL] process_create");
    0
}

fn sys_process_start(_proc_raw: u64, _thread_raw: u64, _entry: u64, _stack: u64, _arg: u64) -> i64 {
    crate::serial_println!("[SYSCALL] process_start");
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
