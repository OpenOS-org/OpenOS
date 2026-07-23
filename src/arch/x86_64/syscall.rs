//! SYSCALL/SYSRET MSR configuration.
//!
//! The `syscall` instruction (fastest user→kernel transition on `x86_64)`:
//!   1. Loads CS from STAR[32:47] (kernel CS), SS = CS + 8
//!   2. Saves RIP → RCX, RFLAGS → R11
//!   3. Masks RFLAGS with FMASK (RFLAGS &= ~FMASK)
//!   4. Jumps to the address in LSTAR
//!
//! The `sysret` instruction (fastest kernel→user transition):
//!   1. Loads CS from STAR[48:63] (user CS), SS = CS + 8
//!   2. Restores RCX → RIP, R11 → RFLAGS
//!
//! This module configures the three MSRs and enables SYSCALL in EFER.

use x86_64::registers::model_specific::{Efer, EferFlags, LStar, SFMask, Star};
use x86_64::registers::rflags::RFlags;
use x86_64::VirtAddr;

use super::gdt;

/// Configure SYSCALL/SYSRET MSRs and enable the SCE (SYSCALL Enable) bit in EFER.
///
/// Must be called after GDT init (needs segment selectors) and before
/// any user-mode transition.
pub fn init() {
    let sel = gdt::selectors();

    // STAR MSR: maps segment selectors for SYSCALL/SYSRET.
    //
    // SYSCALL loads: CS = kernel_code, SS = kernel_data (from STAR[32:47] + 8)
    // SYSRET loads:  CS = user_code,   SS = user_data   (from STAR[48:63] + 8)
    //
    // The x86_64 crate's Star::write handles the bit layout.
    //
    // SAFETY: Writing MSRs is safe because we control the values and do this
    // once during init. Incorrect values would cause #GP on the first SYSCALL.
    Star::write(
        sel.kernel_code,
        sel.kernel_data,
        sel.user_code,
        sel.user_data,
    )
    .expect("Failed to write STAR MSR");

    // LSTAR: the address the CPU jumps to on `syscall`.
    LStar::write(VirtAddr::new(syscall_entry as *const () as u64));

    // FMASK: bits to clear in RFLAGS on SYSCALL. We disable:
    //   - IF (interrupts): must handle syscall atomically before re-enabling
    //   - DF (direction):  C calling convention expects DF=0
    //   - TF (trap):       prevent single-stepping during syscall
    SFMask::write(RFlags::INTERRUPT_FLAG | RFlags::DIRECTION_FLAG | RFlags::TRAP_FLAG);

    // Enable SYSCALL/SYSRET in EFER.
    // SAFETY: Setting the SCE bit, which is required for the SYSCALL instruction.
    let mut efer = Efer::read();
    efer |= EferFlags::SYSTEM_CALL_EXTENSIONS;
    unsafe {
        Efer::write(efer);
    }
}

/// SYSCALL entry point. The CPU jumps here on `syscall`.
///
/// At this point:
///   - RAX = syscall number
///   - RDI, RSI, RDX, R10, R8, R9 = arguments
///   - RCX = user RIP (saved by CPU)
///   - R11 = user RFLAGS (saved by CPU)
///   - RSP = user stack (unchanged)
///   - CS/SS = kernel segments (loaded by CPU from STAR)
///   - Interrupts are disabled (FMASK cleared IF)
///
/// This is a naked function — no prologue/epilogue, we manage the stack
/// entirely with inline assembly.
#[unsafe(naked)]
pub extern "C" fn syscall_entry() {
    // SAFETY: This is a naked function implementing the SYSCALL entry point.
    // The register save/restore sequence matches the SYSCALL convention.
    // We save all registers, call the Rust handler with arguments in the
    // System V ABI registers, then SYSRET back to user-space.
    core::arch::naked_asm!(
        // Save all general-purpose registers. The CPU already saved RIP→RCX
        // and RFLAGS→R11, but we need the rest for the Rust handler.
        "push rcx",       // user RIP (from SYSCALL)
        "push r11",       // user RFLAGS (from SYSCALL)
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",

        // Save syscall arguments passed by user-space.
        "push rax",       // syscall number
        "push rdi",       // arg1
        "push rsi",       // arg2
        "push rdx",       // arg3

        // Call the Rust handler: handle_syscall_raw(number, arg1, arg2, arg3)
        // Per System V ABI: RDI=number, RSI=arg1, RDX=arg2, RCX=arg3
        "mov rdi, rax",           // number
        "mov rsi, [rsp + 8]",    // arg1 (rdi saved above)
        "mov rdx, [rsp + 16]",   // arg2 (rsi saved above)
        "mov rcx, [rsp + 24]",   // arg3 (rdx saved above)
        "call {handler}",

        // Restore registers. RAX now holds the syscall return value.
        "add rsp, 32",    // pop saved rax, rdi, rsi, rdx
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",
        "pop r11",        // user RFLAGS
        "pop rcx",        // user RIP

        // SYSRET: restores RIP from RCX, RFLAGS from R11, switches to user CS/SS.
        "sysretq",
        handler = sym crate::syscall::handle_syscall_raw,
    );
}

/// Raw register state saved on SYSCALL entry. This is what the assembly stub
/// pushes onto the kernel stack before calling the Rust handler.
#[repr(C)]
pub struct SyscallFrame {
    pub rdx: u64, // arg3
    pub rsi: u64, // arg2
    pub rdi: u64, // arg1
    pub rax: u64, // syscall number
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub rbx: u64,
    pub rbp: u64,
    pub r11: u64, // user RFLAGS
    pub rcx: u64, // user RIP
}
