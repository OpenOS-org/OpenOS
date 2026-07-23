//! System call interface.
//!
//! System calls are the mechanism by which user-space tasks request kernel
//! services. On `x86_64`, the canonical approach is the `syscall` instruction,
//! which:
//!   1. Saves RIP → RCX, RFLAGS → R11
//!   2. Loads CS/SS from the STAR MSR
//!   3. Jumps to the handler address loaded from the LSTAR MSR
//!
//! We haven't configured SYSCALL/SYSRET yet — the handler is a placeholder
//! that will be wired up once we have user-space tasks.

use crate::println;

/// System call numbers. `#[repr(u64)]` because the `syscall` instruction
/// passes the number in RAX (a 64-bit register).
#[derive(Debug, Clone, Copy)]
#[repr(u64)]
pub enum SyscallNumber {
    /// Write bytes to stdout (arg1 = buffer ptr, arg2 = length).
    Write = 1,
    /// Read bytes from stdin (arg1 = buffer ptr, arg2 = length).
    Read = 2,
    /// Terminate the calling task.
    Exit = 3,
    /// Create a new task (arg1 = entry point, arg2 = priority).
    Spawn = 4,
    /// Send an IPC message (arg1 = port id, arg2 = message ptr).
    Send = 5,
    /// Receive an IPC message (arg1 = port id, arg2 = buffer ptr).
    Receive = 6,
    /// Voluntarily yield the CPU to the next task.
    Yield = 7,
}

/// Result of a system call. The kernel returns this to user-space in RAX.
#[derive(Debug)]
pub enum SyscallResult {
    /// Success — the u64 is the return value (e.g., bytes read/written).
    Success(u64),
    /// Error — the error code tells user-space what went wrong.
    Error(SyscallError),
}

/// Error codes for system calls.
#[derive(Debug)]
pub enum SyscallError {
    /// Unrecognized syscall number.
    InvalidSyscall,
    /// Argument validation failed.
    InvalidArgument,
    /// Caller lacks the required capability.
    PermissionDenied,
    /// Resource (port, memory, task slot) is unavailable.
    ResourceUnavailable,
}

/// Dispatch a system call. Called from the `syscall` interrupt handler.
///
/// Arguments arrive in registers (convention: RAX=number, RDI/RSI/RDX=args).
/// The `#[must_use]` attribute ensures the caller propagates the result
/// back to user-space — silently discarding a syscall result is a bug.
#[must_use]
pub fn handle_syscall(number: u64, _arg1: u64, _arg2: u64, _arg3: u64) -> SyscallResult {
    match number {
        1 => {
            // TODO: Copy bytes from user-space buffer, write to stdout.
            SyscallResult::Success(0)
        }
        2 => {
            // TODO: Read from stdin into user-space buffer.
            SyscallResult::Success(0)
        }
        3 => {
            println!("[SYSCALL] Task exit requested");
            SyscallResult::Success(0)
        }
        _ => SyscallResult::Error(SyscallError::InvalidSyscall),
    }
}

/// Initialize the syscall interface.
///
/// TODO: Write the LSTAR MSR with the address of our syscall entry point,
/// configure the STAR and FMASK MSR, and enable SCE (SYSCALL Enable) in EFER.
pub fn init() {
    println!("[...] Initializing syscall handler");
    println!("[OK] Syscall handler initialized");
}
