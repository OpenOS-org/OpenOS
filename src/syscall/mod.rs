//! System call interface.
//!
//! User-space invokes a syscall via the `syscall` instruction. The CPU:
//!   1. Saves RIP→RCX, RFLAGS→R11
//!   2. Loads kernel CS/SS from STAR MSR
//!   3. Jumps to LSTAR (our `syscall_entry` in `arch/x86_64/syscall.rs`)
//!
//! The assembly stub saves registers, calls `handle_syscall_raw`, and SYSRETs
//! back to user-space with the return value in RAX.

use crate::println;

/// System call numbers. `#[repr(u64)]` because RAX carries the number.
#[derive(Debug, Clone, Copy)]
#[repr(u64)]
pub enum SyscallNumber {
    /// Write bytes to stdout (arg1 = buffer ptr, arg2 = length).
    Write = 1,
    /// Read bytes from stdin (arg1 = buffer ptr, arg2 = length).
    Read = 2,
    /// Terminate the calling task.
    Exit = 3,
    /// Yield the CPU to the next task.
    Yield = 4,
}

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
        1 => handle_write(arg1, arg2),
        2 => handle_read(arg1, arg2),
        3 => handle_exit(),
        4 => handle_yield(),
        _ => SyscallResult::Error(SyscallError::InvalidSyscall),
    };

    match result {
        SyscallResult::Success(val) => val,
        SyscallResult::Error(err) => err as u64 | (1u64 << 63), // high bit = error flag
    }
}

/// Write bytes to the VGA console. arg1 = pointer to buffer, arg2 = length.
///
/// SAFETY: We read from user-space memory. In a full microkernel we'd validate
/// that the pointer is in user-space range and mapped. For now, we trust the
/// user process since it's kernel-created.
fn handle_write(ptr: u64, len: u64) -> SyscallResult {
    if ptr == 0 || len == 0 {
        return SyscallResult::Error(SyscallError::InvalidArgument);
    }

    let buf = ptr as *const u8;
    for i in 0..len {
        // SAFETY: Reading from user-space buffer. The pointer was validated
        // as non-null above. We print each byte individually to avoid needing
        // to construct a slice (which would require trusting the length).
        let byte = unsafe { *buf.add(i as usize) };
        crate::print!("{}", byte as char);
    }

    SyscallResult::Success(len)
}

/// Read from stdin (placeholder — returns 0 bytes read).
fn handle_read(_ptr: u64, _len: u64) -> SyscallResult {
    // TODO: Implement keyboard input → user buffer
    SyscallResult::Success(0)
}

/// Exit the current task. Halts the CPU since we have no task cleanup yet.
fn handle_exit() -> SyscallResult {
    println!("[SYSCALL] Task exit requested");
    // In a real kernel, we'd remove the task from the scheduler and context-switch.
    // For now, just halt.
    loop {
        x86_64::instructions::hlt();
    }
}

/// Yield the CPU. In a preemptive kernel, this triggers a context switch.
fn handle_yield() -> SyscallResult {
    // TODO: Trigger scheduler to switch to next task
    SyscallResult::Success(0)
}

/// Initialize the syscall interface.
pub fn init() {
    println!("[...] Initializing syscall handler");
    super::arch::x86_64::syscall::init();
    println!("[OK] Syscall handler initialized (LSTAR, STAR, FMASK, EFER.SCE)");
}
