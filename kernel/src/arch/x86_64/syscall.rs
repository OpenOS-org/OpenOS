//! SYSCALL/SYSRET MSR configuration and entry point.
//!
//! The `syscall` instruction:
//!   1. Saves RIP→RCX, RFLAGS→R11
//!   2. Loads CS/SS from STAR MSR
//!   3. Jumps to LSTAR

use x86_64::registers::model_specific::{Efer, EferFlags, LStar, SFMask, Star};
use x86_64::registers::rflags::RFlags;
use x86_64::VirtAddr;

use super::gdt;

/// Configure SYSCALL/SYSRET MSRs and enable the SCE bit in EFER.
pub fn init() {
    let sel = gdt::selectors();
    Star::write(
        sel.user_code,
        sel.user_data,
        sel.kernel_code,
        sel.kernel_data,
    )
    .expect("Failed to write STAR MSR");
    LStar::write(VirtAddr::new(syscall_entry as *const () as u64));
    SFMask::write(RFlags::INTERRUPT_FLAG | RFlags::DIRECTION_FLAG | RFlags::TRAP_FLAG);
    let mut efer = Efer::read();
    efer |= EferFlags::SYSTEM_CALL_EXTENSIONS;
    unsafe {
        Efer::write(efer);
    }
}

/// SYSCALL entry point.
///
/// Saves all registers, calls the Rust handler, restores registers, SYSRETs.
#[unsafe(naked)]
pub extern "C" fn syscall_entry() {
    core::arch::naked_asm!(
        // Save all general-purpose registers.
        "push rcx",       // user RIP (from SYSCALL)
        "push r11",       // user RFLAGS (from SYSCALL)
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "push rax",       // syscall number
        "push rdi",       // arg1
        "push rsi",       // arg2
        "push rdx",       // arg3
        "push r8",        // arg4
        "push r9",        // arg5

        // Call Rust handler: handle_syscall_raw(number, arg1..arg5)
        "mov rdi, [rsp + 40]",   // rax (number)
        "mov rsi, [rsp + 32]",   // rdi (arg1)
        "mov rdx, [rsp + 24]",   // rsi (arg2)
        "mov rcx, [rsp + 16]",   // rdx (arg3)
        "mov r8, [rsp + 8]",     // r8 (arg4)
        "mov r9, [rsp + 0]",     // r9 (arg5)
        "call {handler}",

        // Restore registers. RAX holds the syscall return value.
        "add rsp, 48",          // pop r9,r8,rdx,rsi,rdi,rax
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",
        "pop r11",              // user RFLAGS
        "pop rcx",              // user RIP
        "sysretq",

        handler = sym crate::syscall::handle_syscall_raw,
    );
}
