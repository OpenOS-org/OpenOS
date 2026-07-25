//! System call interface.
//!
//! User-space invokes a syscall via the `syscall` instruction. The CPU:
//!   1. Saves RIP→RCX, RFLAGS→R11
//!   2. Loads kernel CS/SS from STAR MSR
//!   3. Jumps to LSTAR (our `syscall_entry` in `arch/x86_64/syscall.rs`)
//!
//! The assembly stub saves registers, calls `handle_syscall_raw`, and SYSRETs
//! back to user-space with the return value in RAX.

pub mod number;

use number::{
    SYS_COUNT, SYS_EXIT, SYS_PORT_CREATE, SYS_READ, SYS_RECEIVE, SYS_SEND, SYS_WRITE, SYS_YIELD,
};

use crate::{println, serial_print, serial_println};

/// Result of a system call. Returned to user-space in RAX.
#[derive(Debug)]
pub enum SyscallResult {
    /// Success with return value.
    Success(u64),
    /// Error with error code.
    Error(SyscallError),
}

/// Error codes for system calls.
#[derive(Debug)]
pub enum SyscallError {
    /// Unrecognized syscall number.
    InvalidSyscall = 1,
    /// Argument validation failed.
    InvalidArgument = 2,
    /// Caller lacks the required capability.
    PermissionDenied = 3,
}

/// Raw syscall handler called from the assembly stub.
///
/// This is `extern "C"` because it's called from `syscall_entry` assembly
/// using the System V ABI. Arguments arrive in registers:
///   RDI = syscall number, RSI = arg1, RDX = arg2, RCX = arg3
///
/// Returns the syscall result value in RAX.
#[no_mangle]
pub extern "C" fn handle_syscall_raw(number: u64, arg1: u64, arg2: u64, _arg3: u64) -> u64 {
    let result = match number {
        SYS_WRITE => handle_write(arg1, arg2),
        SYS_READ => handle_read(arg1, arg2),
        SYS_EXIT => handle_exit(arg1),
        SYS_YIELD => handle_yield(),
        SYS_PORT_CREATE => handle_port_create(),
        SYS_SEND => handle_send(arg1, arg2),
        SYS_RECEIVE => handle_receive(arg1, arg2),
        _ => SyscallResult::Error(SyscallError::InvalidSyscall),
    };

    match result {
        SyscallResult::Success(val) => val,
        SyscallResult::Error(err) => err as u64 | (1u64 << 63),
    }
}

/// Maximum user-space address. Pointers above this are in kernel space
/// and must not be dereferenced on behalf of user code.
const USER_SPACE_MAX: u64 = 0x0000_8000_0000_0000;

/// Validate that a user-space pointer is within the user-space range.
///
/// Returns `true` if the pointer (with `len` bytes) is entirely within
/// user-space. This prevents user code from reading kernel memory through
/// syscalls.
fn is_valid_user_ptr(ptr: u64, len: u64) -> bool {
    ptr > 0 && len > 0 && ptr < USER_SPACE_MAX && ptr.saturating_add(len) <= USER_SPACE_MAX
}

/// Write bytes to the VGA console and serial port.
///
/// Writes exactly the user's bytes — no prefix, no suffix. Both VGA and
/// serial receive identical output so the user can rely on the write being
/// transparent.
///
/// Returns `InvalidArgument` if the buffer is null, zero-length, or
/// extends beyond user-space.
fn handle_write(ptr: u64, len: u64) -> SyscallResult {
    if !is_valid_user_ptr(ptr, len) {
        return SyscallResult::Error(SyscallError::InvalidArgument);
    }

    let buf = ptr as *const u8;
    for i in 0..len {
        // SAFETY: The pointer was validated as a valid user-space address above.
        let byte = unsafe { *buf.add(i as usize) };
        crate::print!("{}", byte as char);
        serial_print!("{}", byte as char);
    }

    SyscallResult::Success(len)
}

/// Read from stdin (placeholder — returns 0 bytes read).
fn handle_read(_ptr: u64, _len: u64) -> SyscallResult {
    // TODO: Implement keyboard input → user buffer
    SyscallResult::Success(0)
}

/// Exit the current task.
///
/// In a full microkernel this would clean up the task's resources (ports,
/// memory mappings, file descriptors) and remove it from the scheduler.
/// For now we log the exit code and halt.
fn handle_exit(status: u64) -> SyscallResult {
    println!("[SYS_EXIT] Task exited with status {status}");
    serial_println!("[SYS_EXIT] Task exited with status {status}");
    loop {
        x86_64::instructions::hlt();
    }
}

/// Yield the CPU to the next task.
fn handle_yield() -> SyscallResult {
    SyscallResult::Success(0)
}

/// Create a new IPC port.
fn handle_port_create() -> SyscallResult {
    let port_id = crate::ipc::create_port();
    SyscallResult::Success(port_id)
}

/// Send an IPC message.
fn handle_send(_port_id: u64, _msg_ptr: u64) -> SyscallResult {
    // TODO: Copy message from user-space, validate port ID, deliver
    SyscallResult::Error(SyscallError::InvalidSyscall)
}

/// Receive an IPC message.
fn handle_receive(_port_id: u64, _buf_ptr: u64) -> SyscallResult {
    // TODO: Validate port ID, dequeue message, copy to user-space
    SyscallResult::Error(SyscallError::InvalidSyscall)
}

/// Initialize the syscall interface.
pub fn init() {
    println!("[...] Initializing syscall handler");
    serial_println!("[...] Initializing syscall handler");
    super::arch::x86_64::syscall::init();
    println!("[OK] Syscall handler initialized (LSTAR, STAR, FMASK, EFER.SCE)");
    serial_println!("[OK] Syscall handler initialized (LSTAR, STAR, FMASK, EFER.SCE)");
}
