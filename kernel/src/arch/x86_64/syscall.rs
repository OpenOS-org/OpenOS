//! SYSCALL/SYSRET MSR configuration and entry point.
//!
//! The `syscall` instruction:
//!   1. Saves RIP→RCX, RFLAGS→R11
//!   2. Loads CS/SS from STAR MSR
//!   3. Jumps to LSTAR
//!
//! Context switch support: after the Rust handler returns, if `SWITCH_CONTEXT`
//! is non-null, the stub restores that context instead of the saved registers.

use x86_64::registers::model_specific::{Efer, EferFlags, LStar, SFMask, Star};
use x86_64::registers::rflags::RFlags;
use x86_64::VirtAddr;

use super::gdt;

/// When non-null, the syscall exit path restores this context instead of
/// the saved registers on the stack. Set by `block_and_switch` in the
/// scheduler when a context switch is needed.
#[no_mangle]
pub static mut SWITCH_CONTEXT: *const crate::task::task::SavedContext = core::ptr::null();

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

/// SYSCALL entry point with context switch support.
///
/// Saves all registers, calls the Rust handler, then either:
///   - Normal return: restores saved registers, SYSRETs to caller
///   - Context switch: restores `SWITCH_CONTEXT`, SYSRETs to new task
#[unsafe(naked)]
pub extern "C" fn syscall_entry() {
    core::arch::naked_asm!(
        // Save all general-purpose registers on the kernel stack.
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

        // Handler returned. RAX = syscall result.
        // Check if a context switch is needed.
        "push rax",               // save return value
        "lea rax, [rip + {switch_ptr}]",
        "mov rax, [rax]",
        "test rax, rax",
        "jnz .Lswitch",

        // Normal return: restore saved registers.
        "pop rax",                // restore return value
        "add rsp, 48",            // pop r9,r8,rdx,rsi,rdi,rax
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",
        "pop r11",                // user RFLAGS
        "pop rcx",                // user RIP
        "sysretq",

        // Context switch: restore from SWITCH_CONTEXT.
        // RAX = pointer to SavedContext.
        ".Lswitch:",
        "pop rbx",                // discard saved return value
        // Clear SWITCH_CONTEXT.
        "lea rcx, [rip + {switch_ptr}]",
        "mov qword ptr [rcx], 0",
        // Restore registers from the new context.
        // SavedContext layout: r9, r8, rdx, rsi, rdi, rax, r15..rbx, rbp, r11, rcx, rsp
        "mov r9,  [rax + 0]",
        "mov r8,  [rax + 8]",
        "mov rdx, [rax + 16]",
        "mov rsi, [rax + 24]",
        "mov rdi, [rax + 32]",
        // rax restored last (it holds the context pointer)
        "mov r15, [rax + 48]",
        "mov r14, [rax + 56]",
        "mov r13, [rax + 64]",
        "mov r12, [rax + 72]",
        "mov rbx, [rax + 80]",
        "mov rbp, [rax + 88]",
        "mov r11, [rax + 96]",    // new task's RFLAGS
        "mov rcx, [rax + 104]",   // new task's RIP
        "mov rsp, [rax + 112]",   // new task's RSP
        "mov rax, [rax + 40]",    // new task's RAX
        "sysretq",

        switch_ptr = sym SWITCH_CONTEXT,
        handler = sym crate::syscall::handle_syscall_raw,
    );
}
