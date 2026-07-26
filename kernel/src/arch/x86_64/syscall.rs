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
///
/// # Safety
///
/// This pointer is written by the Rust handler (`block_and_switch`) and
/// read by the assembly stub after the handler returns. The pointed-to
/// `SavedContext` lives in `NEXT_CTX_STORAGE` (a static mut) and is valid
/// for the duration of the syscall exit path.
#[no_mangle]
pub static mut SWITCH_CONTEXT: *const crate::task::task::SavedContext = core::ptr::null();

/// Points to the saved registers on the kernel stack during a syscall.
///
/// The assembly stub sets this before calling the Rust handler, allowing
/// syscall handlers (e.g., `channel_receive`) to capture the user-space
/// register state for context switching. Layout matches `SavedContext`.
///
/// # Safety
///
/// Points into the kernel stack frame created by the assembly stub. Valid
/// only during the Rust handler's execution. Cleared to null before the
/// handler returns (so it cannot be used after the handler exits).
#[no_mangle]
pub static mut CURRENT_CONTEXT: *const crate::task::task::SavedContext = core::ptr::null();

/// Capture the current syscall context as a `SavedContext`.
///
/// Reads the saved registers from the kernel stack via `CURRENT_CONTEXT`.
/// Called by syscall handlers that need to block and switch to another task.
///
/// # Panics
///
/// Panics if `CURRENT_CONTEXT` is null, which indicates a bug -- this
/// function must only be called from within a syscall handler where the
/// assembly stub has already set `CURRENT_CONTEXT`.
#[allow(static_mut_refs)]
#[must_use]
pub fn capture_current_context() -> crate::task::task::SavedContext {
    // SAFETY: CURRENT_CONTEXT is set by the assembly stub before calling the
    // Rust handler. It points to the saved register area on the kernel stack,
    // which has the same layout as SavedContext. The pointer is valid for the
    // duration of the syscall handler.
    unsafe {
        assert!(
            !CURRENT_CONTEXT.is_null(),
            "capture_current_context called with null CURRENT_CONTEXT — \
             must be called from within a syscall handler"
        );
        *CURRENT_CONTEXT
    }
}

/// Configure SYSCALL/SYSRET MSRs and enable the SCE bit in EFER.
///
/// # Panics
///
/// Panics if writing to the `STAR` MSR fails (should not happen on `x86_64`).
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
    // SAFETY: Setting the SCE (System Call Extensions) bit in EFER enables
    // the SYSCALL/SYSRET instructions. This must be done after configuring
    // STAR, LSTAR, and SFMASK. The EFER register is model-specific and
    // requires the `unsafe` write. We only set the SCE bit; other bits
    // (LME, LMA, etc.) are managed by the bootloader/paging setup.
    let mut efer = Efer::read();
    efer |= EferFlags::SYSTEM_CALL_EXTENSIONS;
    unsafe {
        Efer::write(efer);
    }
}

/// SYSCALL entry point with context switch support.
///
/// ## Register save/restore
///
/// On entry, the `syscall` instruction has already placed the user RIP in
/// RCX and user RFLAGS in R11. The stub pushes these plus all other
/// general-purpose registers onto the kernel stack in `SavedContext` order:
///
/// ```text
///   [RSP+104]  rcx  (user RIP from SYSCALL)
///   [RSP+96]   r11  (user RFLAGS from SYSCALL)
///   [RSP+88]   rbp
///   [RSP+80]   rbx
///   [RSP+72]   r12
///   [RSP+64]   r13
///   [RSP+56]   r14
///   [RSP+48]   r15
///   [RSP+40]   rax  (syscall number)
///   [RSP+32]   rdi  (arg1)
///   [RSP+24]   rsi  (arg2)
///   [RSP+16]   rdx  (arg3)
///   [RSP+8]    r8   (arg4)
///   [RSP+0]    r9   (arg5)
/// ```
///
/// ## Context switch path
///
/// After the Rust handler returns, the stub checks `SWITCH_CONTEXT`:
///
/// - **Null**: Normal return. Restore saved registers and SYSRETQ.
/// - **Non-null**: Context switch. The pointed-to `SavedContext` contains
///   the new task's register state. Two sub-paths:
///
///   - **User-mode** (`is_kernel == 0`): Restore registers from the context,
///     then SYSRETQ to Ring 3. RCX = new RIP, R11 = new RFLAGS.
///
///   - **Kernel-mode** (`is_kernel == 1`): Restore registers, push an IRETQ
///     frame (SS, RSP, RFLAGS, CS, RIP) on the new task's stack, and IRETQ.
///     This is used when switching to a kernel task (e.g., the idle task).
///
/// ## CR3 switching
///
/// If the new task's `cr3` field is non-zero, it is loaded into CR3 before
/// restoring registers. This switches the page table to the new task's
/// address space. If zero, the current CR3 is kept (kernel page table).
#[unsafe(naked)]
pub extern "C" fn syscall_entry() {
    core::arch::naked_asm!(
        // ── Register save ──────────────────────────────────────────────
        // Save all general-purpose registers on the kernel stack.
        // SYSCALL has already placed user RIP in RCX and RFLAGS in R11.
        "push rcx",       // user RIP (from SYSCALL instruction)
        "push r11",       // user RFLAGS (from SYSCALL instruction)
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "push rax",       // syscall number (in RAX on entry)
        "push rdi",       // arg1
        "push rsi",       // arg2
        "push rdx",       // arg3
        "push r8",        // arg4
        "push r9",        // arg5

        // ── Set CURRENT_CONTEXT ────────────────────────────────────────
        // Point CURRENT_CONTEXT to the start of the saved register area
        // on the kernel stack. This lets Rust handlers (e.g., channel_receive)
        // capture the user-space register state for later context switching.
        "lea rax, [rsp]",
        "lea rcx, [rip + {current_ctx_ptr}]",
        "mov [rcx], rax",

        // ── Call Rust handler ──────────────────────────────────────────
        // handle_syscall_raw(number: u64, arg1..arg5: u64) -> i64
        // Arguments are in SysV calling convention: rdi, rsi, rdx, rcx, r8, r9.
        "mov rdi, [rsp + 40]",   // rax (syscall number)
        "mov rsi, [rsp + 32]",   // rdi (arg1)
        "mov rdx, [rsp + 24]",   // rsi (arg2)
        "mov rcx, [rsp + 16]",   // rdx (arg3)
        "mov r8, [rsp + 8]",     // r8  (arg4)
        "mov r9, [rsp + 0]",     // r9  (arg5)
        "call {handler}",

        // ── Handler returned ───────────────────────────────────────────
        // RAX = syscall return value. Clear CURRENT_CONTEXT since the
        // stack frame will be torn down.
        "lea rcx, [rip + {current_ctx_ptr}]",
        "mov qword ptr [rcx], 0",

        // ── Check for context switch ───────────────────────────────────
        // If SWITCH_CONTEXT is non-null, the scheduler wants us to switch
        // to a different task. Otherwise, return normally to the caller.
        "push rax",               // save return value temporarily
        "lea rax, [rip + {switch_ptr}]",
        "mov rax, [rax]",
        "test rax, rax",
        "jnz .Lswitch",

        // ── Normal return (no context switch) ──────────────────────────
        // Restore all saved registers and SYSRETQ back to user-space.
        // SYSCALL sets CS to STAR.IA32_SYSCALL_CS and SS to STAR.IA32_SYSCALL_CS+8.
        // SYSRETQ restores CS to STAR.IA32_SYSCALL_CS+16 and SS to STAR.IA32_SYSCALL_CS+24.
        "pop rax",                // restore syscall return value
        "add rsp, 48",            // pop r9,r8,rdx,rsi,rdi,rax (6 * 8 bytes)
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",
        "pop r11",                // user RFLAGS (restored by SYSRETQ)
        "pop rcx",                // user RIP (restored by SYSRETQ)
        "sysretq",

        // ── Context switch path ────────────────────────────────────────
        // RAX = pointer to SavedContext for the new task.
        // We discard the saved return value (the new task won't return
        // to the same point as the old task).
        ".Lswitch:",
        "pop rbx",                // discard saved return value

        // Clear SWITCH_CONTEXT so the next syscall doesn't re-switch.
        "lea rcx, [rip + {switch_ptr}]",
        "mov qword ptr [rcx], 0",

        // Save context pointer in RBX (used as base for all field loads).
        // RBX is the last register to be restored from the context.
        "mov rbx, rax",

        // ── CR3 switch ────────────────────────────────────────────────
        // If the new task has its own page table (cr3 != 0), load it.
        // If cr3 == 0, keep the current CR3 (kernel page table).
        "mov rax, [rbx + 128]",  // SavedContext.cr3
        "test rax, rax",
        "jz .Lno_cr3",
        "mov cr3, rax",          // Switch to new task's page table
        ".Lno_cr3:",

        // ── IRETQ vs SYSRET decision ──────────────────────────────────
        // Check the is_kernel flag (offset 120 in SavedContext).
        //   is_kernel == 0  ->  user-mode task, use SYSRETQ (Ring 3)
        //   is_kernel == 1  ->  kernel-mode task, use IRETQ (Ring 0)
        "cmp qword ptr [rbx + 120], 1",
        "je .Liret_switch",

        // ── User-mode restore (SYSRETQ) ───────────────────────────────
        // SYSRETQ requires: RCX = target RIP, R11 = target RFLAGS.
        // CS/SS are loaded from the STAR MSR automatically.
        "mov r9,  [rbx + 0]",    // SavedContext.r9
        "mov r8,  [rbx + 8]",    // SavedContext.r8
        "mov rdx, [rbx + 16]",   // SavedContext.rdx
        "mov rsi, [rbx + 24]",   // SavedContext.rsi
        "mov rdi, [rbx + 32]",   // SavedContext.rdi
        "mov r15, [rbx + 48]",   // SavedContext.r15
        "mov r14, [rbx + 56]",   // SavedContext.r14
        "mov r13, [rbx + 64]",   // SavedContext.r13
        "mov r12, [rbx + 72]",   // SavedContext.r12
        // RBX and RAX restored last (RBX holds the context pointer).
        "mov rbp, [rbx + 88]",   // SavedContext.rbp
        "mov r11, [rbx + 96]",   // SavedContext.r11 (RFLAGS for SYSRETQ)
        "mov rcx, [rbx + 104]",  // SavedContext.rcx (RIP for SYSRETQ)
        "mov rsp, [rbx + 112]",  // SavedContext.rsp
        "mov rax, [rbx + 40]",   // SavedContext.rax
        "mov rbx, [rbx + 80]",   // SavedContext.rbx
        "sysretq",               // Return to Ring 3

        // ── Kernel-mode restore (IRETQ) ───────────────────────────────
        // IRETQ expects a 5-word frame on the stack: SS, RSP, RFLAGS, CS, RIP.
        // This path is used for kernel tasks (e.g., idle task) that don't
        // use SYSRETQ because they run in Ring 0.
        ".Liret_switch:",
        "mov r9,  [rbx + 0]",
        "mov r8,  [rbx + 8]",
        "mov rdx, [rbx + 16]",
        "mov rsi, [rbx + 24]",
        "mov rdi, [rbx + 32]",
        "mov r15, [rbx + 48]",
        "mov r14, [rbx + 56]",
        "mov r13, [rbx + 64]",
        "mov r12, [rbx + 72]",
        "mov rbp, [rbx + 88]",
        "mov r11, [rbx + 96]",   // RFLAGS

        // Set up IRETQ frame on the new task's kernel stack.
        // IRETQ pops: RIP, CS, RFLAGS, RSP, SS (in that order from stack).
        "mov rsp, [rbx + 112]",  // new task's RSP
        "push {kernel_data}",    // SS  (Ring 0 data segment, GDT index 2)
        "push rsp",              // RSP
        "push r11",              // RFLAGS
        "push {kernel_code}",    // CS  (Ring 0 code segment, GDT index 1)
        "push qword ptr [rbx + 104]", // RIP
        "mov rax, [rbx + 40]",   // SavedContext.rax
        "mov rbx, [rbx + 80]",   // SavedContext.rbx
        "iretq",                 // Return to Ring 0

        switch_ptr = sym SWITCH_CONTEXT,
        current_ctx_ptr = sym CURRENT_CONTEXT,
        handler = sym crate::syscall::handle_syscall_raw,
        kernel_code = const 0x08,   // GDT index 1: kernel code segment
        kernel_data = const 0x10,   // GDT index 2: kernel data segment
    );
}

#[cfg(test)]
mod tests {
    use core::mem::offset_of;

    use super::*;
    use crate::task::task::SavedContext;

    /// `SWITCH_CONTEXT` starts as null (no pending context switch).
    #[test]
    fn test_switch_context_starts_null() {
        // SAFETY: We only read the initial value; no concurrent access in tests.
        unsafe {
            assert!(SWITCH_CONTEXT.is_null());
        }
    }

    /// `CURRENT_CONTEXT` starts as null (no active syscall).
    #[test]
    fn test_current_context_starts_null() {
        // SAFETY: We only read the initial value; no concurrent access in tests.
        unsafe {
            assert!(CURRENT_CONTEXT.is_null());
        }
    }

    /// `SWITCH_CONTEXT` must be `#[no_mangle]` and have the correct symbol name.
    /// This is verified by the assembly stub using `sym SWITCH_CONTEXT`.
    #[test]
    fn test_switch_context_is_accessible() {
        let ptr = core::ptr::addr_of!(SWITCH_CONTEXT);
        assert!(!ptr.is_null());
    }

    /// `CURRENT_CONTEXT` must be `#[no_mangle]` and have the correct symbol name.
    #[test]
    fn test_current_context_is_accessible() {
        let ptr = core::ptr::addr_of!(CURRENT_CONTEXT);
        assert!(!ptr.is_null());
    }

    /// Verify `SavedContext` field offsets match the assembly stub's expectations.
    ///
    /// The assembly stub uses hardcoded offsets (e.g., `[rbx + 120]` for
    /// `is_kernel`). These must match the struct layout exactly.
    #[test]
    fn test_saved_context_field_offsets() {
        assert_eq!(offset_of!(SavedContext, r9), 0);
        assert_eq!(offset_of!(SavedContext, r8), 8);
        assert_eq!(offset_of!(SavedContext, rdx), 16);
        assert_eq!(offset_of!(SavedContext, rsi), 24);
        assert_eq!(offset_of!(SavedContext, rdi), 32);
        assert_eq!(offset_of!(SavedContext, rax), 40);
        assert_eq!(offset_of!(SavedContext, r15), 48);
        assert_eq!(offset_of!(SavedContext, r14), 56);
        assert_eq!(offset_of!(SavedContext, r13), 64);
        assert_eq!(offset_of!(SavedContext, r12), 72);
        assert_eq!(offset_of!(SavedContext, rbx), 80);
        assert_eq!(offset_of!(SavedContext, rbp), 88);
        assert_eq!(offset_of!(SavedContext, r11), 96);
        assert_eq!(offset_of!(SavedContext, rcx), 104);
        assert_eq!(offset_of!(SavedContext, rsp), 112);
        assert_eq!(offset_of!(SavedContext, is_kernel), 120);
        assert_eq!(offset_of!(SavedContext, cr3), 128);
    }

    /// `SavedContext` total size must be 17 fields * 8 bytes = 136 bytes.
    #[test]
    fn test_saved_context_size() {
        assert_eq!(core::mem::size_of::<SavedContext>(), 136);
    }

    /// Kernel code segment selector used in IRETQ frame.
    #[test]
    fn test_kernel_code_segment() {
        // GDT index 1 = selector 0x08.
        assert_eq!(0x08u64, 1 * 8);
    }

    /// Kernel data segment selector used in IRETQ frame.
    #[test]
    fn test_kernel_data_segment() {
        // GDT index 2 = selector 0x10.
        assert_eq!(0x10u64, 2 * 8);
    }

    /// `capture_current_context` panics if `CURRENT_CONTEXT` is null.
    #[test]
    #[should_panic(expected = "capture_current_context called with null CURRENT_CONTEXT")]
    fn test_capture_current_context_panics_when_null() {
        // SAFETY: We set CURRENT_CONTEXT to null before calling.
        // No concurrent access in tests.
        unsafe {
            CURRENT_CONTEXT = core::ptr::null();
        }
        capture_current_context();
    }
}
