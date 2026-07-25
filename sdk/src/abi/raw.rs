//! Raw syscall invocation via the `syscall` instruction.
//!
//! This is the lowest-level interface — all higher-level SDK functions
//! ultimately call these. The `syscall` instruction:
//!   1. Saves RIP → RCX, RFLAGS → R11 (CPU does this automatically)
//!   2. Loads kernel CS/SS from STAR MSR
//!   3. Jumps to LSTAR (kernel's syscall entry point)
//!
//! # Safety
//!
//! All functions are `unsafe` because passing invalid arguments to the kernel
//! can cause undefined behavior (e.g., a bad pointer in `SYS_WRITE` will crash).

/// Invoke a syscall with 0 arguments.
///
/// # Safety
/// `n` must be a valid syscall number.
#[must_use]
#[inline]
pub unsafe fn syscall0(n: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") n => ret,
        out("rcx") _,
        out("r11") _,
        options(nostack)
    );
    ret
}

/// Invoke a syscall with 1 argument.
///
/// # Safety
/// `n` must be a valid syscall number. `a1` must be valid for the syscall.
#[must_use]
#[inline]
pub unsafe fn syscall1(n: u64, a1: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") n => ret,
        in("rdi") a1,
        out("rcx") _,
        out("r11") _,
        options(nostack)
    );
    ret
}

/// Invoke a syscall with 2 arguments.
///
/// # Safety
/// `n` must be a valid syscall number. Arguments must be valid for the syscall.
#[must_use]
#[inline]
pub unsafe fn syscall2(n: u64, a1: u64, a2: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") n => ret,
        in("rdi") a1,
        in("rsi") a2,
        out("rcx") _,
        out("r11") _,
        options(nostack)
    );
    ret
}

/// Invoke a syscall with 3 arguments.
///
/// # Safety
/// `n` must be a valid syscall number. Arguments must be valid for the syscall.
#[must_use]
#[inline]
pub unsafe fn syscall3(n: u64, a1: u64, a2: u64, a3: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") n => ret,
        in("rdi") a1,
        in("rsi") a2,
        in("rdx") a3,
        out("rcx") _,
        out("r11") _,
        options(nostack)
    );
    ret
}
