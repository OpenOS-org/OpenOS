//! Process control operations.
//!
//! Provides wrappers for process lifecycle syscalls: exit and yield.

use crate::abi::{number, raw};

/// Terminate the current process with the given exit code.
///
/// This function does not return. The kernel reclaims all resources
/// associated with the process.
///
/// Convention: `exit(0)` indicates success, non-zero indicates failure.
pub fn exit(status: i32) -> ! {
    #[allow(clippy::cast_sign_loss)]
    let code = status as u64;
    unsafe {
        let _ = raw::syscall1(number::SYS_EXIT, code);
    }
    // The kernel halts the CPU — we should never reach here.
    loop {
        unsafe { core::arch::asm!("hlt") }
    }
}

/// Yield the CPU to the next task in the scheduler.
///
/// This is a cooperative scheduling hint. With preemptive scheduling,
/// the timer interrupt will preempt regardless.
pub fn yield_() {
    unsafe {
        let _ = raw::syscall0(number::SYS_YIELD);
    }
}
