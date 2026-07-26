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

/// Copy bytes from user-space into a kernel buffer.
///
/// # Safety
/// `src` must be a valid user-space pointer with at least `len` readable bytes.
unsafe fn copy_from_user(src: *const u8, len: usize) -> Option<alloc::vec::Vec<u8>> {
    if src.is_null() || len == 0 || len > 4096 {
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
/// `dst` must be a valid user-space pointer with at least `len` writable bytes.
unsafe fn copy_to_user(dst: *mut u8, src: &[u8]) -> bool {
    let len = src.len();
    if dst.is_null() || len == 0 || len > 4096 {
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
        // --- Channel ---
        SYS_CHANNEL_CREATE => handle_channel_create(),
        SYS_CHANNEL_SEND => handle_channel_send(arg1, arg2, arg3),
        SYS_CHANNEL_RECEIVE => handle_channel_receive(arg1, arg2, arg3),
        SYS_CHANNEL_CALL => handle_channel_call(arg1, arg2, arg3, arg4, arg5),
        SYS_CHANNEL_REPLY => handle_channel_reply(arg1, arg2, arg3),

        // --- Handle ---
        SYS_HANDLE_CLOSE => handle_handle_close(arg1),
        SYS_HANDLE_DUPLICATE => handle_handle_duplicate(arg1, arg2),
        SYS_HANDLE_TRANSFER => handle_handle_transfer(arg1, arg2, arg3),

        // --- Process ---
        SYS_PROCESS_CREATE => handle_process_create(arg1, arg2, arg3),
        SYS_PROCESS_START => handle_process_start(arg1, arg2, arg3, arg4, arg5),
        SYS_PROCESS_EXIT => handle_process_exit(arg1),
        SYS_PROCESS_WAIT => handle_process_wait(arg1, arg2),

        // --- Thread ---
        SYS_THREAD_CREATE => handle_thread_create(arg1),
        SYS_THREAD_EXIT => handle_thread_exit(),
        SYS_THREAD_YIELD => handle_thread_yield(),

        // --- OpenOS debug ---
        SYS_CONSOLE_WRITE => handle_console_write(arg1, arg2),

        _ => Error::UnknownSyscall as i64,
    }
}

// ─────────────────── Channel handlers ───────────────────

fn handle_channel_create() -> i64 {
    // TODO: insert both ends into the calling task's handle table
    // For now, return a placeholder.
    crate::serial_println!("[SYSCALL] channel_create");
    0
}

fn handle_channel_send(handle_raw: u64, msg_ptr: u64, msg_len: u64) -> i64 {
    let _handle = Handle::from_raw(handle_raw);
    let Some(msg) = (unsafe { copy_from_user(msg_ptr as *const u8, msg_len as usize) }) else {
        return Error::BadPointer as i64;
    };
    crate::serial_println!("[SYSCALL] channel_send: {} bytes", msg.len());
    // TODO: look up handle in task's handle table, call channel.send()
    0
}

fn handle_channel_receive(handle_raw: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    let _handle = Handle::from_raw(handle_raw);
    crate::serial_println!("[SYSCALL] channel_receive: buf_len={buf_len}");
    // TODO: look up handle, call channel.receive()
    Error::WouldBlock as i64
}

fn handle_channel_call(
    handle_raw: u64,
    msg_ptr: u64,
    msg_len: u64,
    reply_ptr: u64,
    reply_len: u64,
) -> i64 {
    let _handle = Handle::from_raw(handle_raw);
    let Some(msg) = (unsafe { copy_from_user(msg_ptr as *const u8, msg_len as usize) }) else {
        return Error::BadPointer as i64;
    };
    crate::serial_println!(
        "[SYSCALL] channel_call: {} bytes, reply_buf={:#x}",
        msg.len(),
        reply_ptr
    );
    // TODO: look up handle, call channel.call(), block task, return reply
    let _ = (reply_ptr, reply_len);
    Error::WouldBlock as i64
}

fn handle_channel_reply(handle_raw: u64, msg_ptr: u64, msg_len: u64) -> i64 {
    let _handle = Handle::from_raw(handle_raw);
    let Some(msg) = (unsafe { copy_from_user(msg_ptr as *const u8, msg_len as usize) }) else {
        return Error::BadPointer as i64;
    };
    crate::serial_println!("[SYSCALL] channel_reply: {} bytes", msg.len());
    // TODO: look up handle, call channel.reply(), unblock caller
    0
}

// ─────────────────── Handle handlers ───────────────────

fn handle_handle_close(handle_raw: u64) -> i64 {
    let _handle = Handle::from_raw(handle_raw);
    crate::serial_println!("[SYSCALL] handle_close");
    // TODO: remove from task's handle table
    0
}

fn handle_handle_duplicate(handle_raw: u64, new_rights: u64) -> i64 {
    let _handle = Handle::from_raw(handle_raw);
    let _rights = Rights::from_raw(new_rights as u16);
    crate::serial_println!("[SYSCALL] handle_duplicate");
    // TODO: duplicate handle with narrowed rights
    0
}

fn handle_handle_transfer(handle_raw: u64, channel_raw: u64, rights: u64) -> i64 {
    let _handle = Handle::from_raw(handle_raw);
    let _channel = Handle::from_raw(channel_raw);
    let _rights = Rights::from_raw(rights as u16);
    crate::serial_println!("[SYSCALL] handle_transfer");
    // TODO: transfer handle through channel
    0
}

// ─────────────────── Process handlers ───────────────────

fn handle_process_create(_job_raw: u64, name_ptr: u64, name_len: u64) -> i64 {
    let _name = unsafe { copy_from_user(name_ptr as *const u8, name_len as usize) };
    crate::serial_println!("[SYSCALL] process_create");
    // TODO: create process, return (proc_handle, vmar_handle)
    0
}

fn handle_process_start(
    _proc_raw: u64,
    _thread_raw: u64,
    _entry: u64,
    _stack: u64,
    _arg: u64,
) -> i64 {
    crate::serial_println!("[SYSCALL] process_start");
    // TODO: start thread in process
    0
}

fn handle_process_exit(status: u64) -> i64 {
    crate::serial_println!("[SYS_EXIT] status={status}");
    loop {
        x86_64::instructions::hlt();
    }
}

fn handle_process_wait(_proc_raw: u64, _timeout: u64) -> i64 {
    crate::serial_println!("[SYSCALL] process_wait");
    // TODO: block until process exits
    Error::WouldBlock as i64
}

// ─────────────────── Thread handlers ───────────────────

fn handle_thread_create(_proc_raw: u64) -> i64 {
    crate::serial_println!("[SYSCALL] thread_create");
    0
}

fn handle_thread_exit() -> i64 {
    crate::serial_println!("[SYSCALL] thread_exit");
    loop {
        x86_64::instructions::hlt();
    }
}

fn handle_thread_yield() -> i64 {
    0
}

// ─────────────────── Debug console ───────────────────

fn handle_console_write(ptr: u64, len: u64) -> i64 {
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
