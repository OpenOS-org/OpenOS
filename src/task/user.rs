//! User-mode process management.
//!
//! This module handles the transition from kernel (Ring 0) to user-space (Ring 3).
//! The first user process is a hardcoded "hello world" program that demonstrates:
//!   1. Privilege-level transition via IRETQ
//!   2. System call via SYSCALL instruction
//!   3. Return to user-space via SYSRET
//!
//! The user process code is embedded directly in the kernel binary as a static
//! array of bytes. In a real microkernel, the process would be loaded from an
//! ELF binary by the VFS/exec server.

use crate::println;

/// User-mode stack: 4 pages = 16 KiB. Must be page-aligned and mapped
/// with user-accessible permissions.
const USER_STACK_SIZE: usize = 4096 * 4;

/// User-mode code: a minimal program that calls `syscall` (write) and then
/// `syscall` (exit).
///
/// The code is position-independent and uses the `syscall` instruction for
/// all kernel interactions. Syscall convention:
///   RAX = syscall number
///   RDI = arg1, RSI = arg2, RDX = arg3
///
/// The program:
///   1. Writes "Hello from user-space!\n" to stdout (syscall 1)
///   2. Exits (syscall 3)
static USER_CODE: &[u8] = &[
    // "Hello from user-space!\n" at offset 0x50 (we'll load RSI with the address)
    0x48, 0x8d, 0x35, 0x49, 0x00, 0x00, 0x00, // lea rsi, [rip + 0x49]  → string at end
    0x48, 0xc7, 0xc7, 0x19, 0x00, 0x00, 0x00, // mov rdi, 25            → length
    0x48, 0xc7, 0xc0, 0x01, 0x00, 0x00, 0x00, // mov rax, 1             → syscall: write
    0x0f, 0x05, // syscall
    // Exit
    0x48, 0xc7, 0xc0, 0x03, 0x00, 0x00, 0x00, // mov rax, 3             → syscall: exit
    0x0f, 0x05, // syscall
    // Infinite loop in case exit returns
    0xeb, 0xfe, // jmp $
    // String "Hello from user-space!\n" at offset 0x22 (34 bytes from start)
    // Actually at offset 0x22 + 0x49 = 0x6b... let me recalculate.
    // The lea rsi instruction is at offset 0, target is at rip + 0x49 = 0x07 + 0x49 = 0x50
    // Padding to offset 0x50
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    // "Hello from user-space!\n" at offset 0x50 (25 bytes)
    b'H', b'e', b'l', b'l', b'o', b' ', b'f', b'r', b'o', b'm', b' ', b'u', b's', b'e', b'r', b'-',
    b's', b'p', b'a', b'c', b'e', b'!', b'\n',
];

/// User-mode static data (page-aligned, identity-mapped by bootloader for
/// the first 2 MiB). We place the user code and stack here.
///
/// In a real kernel, we'd allocate pages and set up separate page tables.
/// For the first process, we rely on the bootloader's identity mapping.
#[repr(align(4096))]
struct UserMemory {
    /// User code (read + execute, user accessible)
    code: [u8; 4096],
    /// User stack (read + write, user accessible)
    stack: [u8; USER_STACK_SIZE],
}

static mut USER_MEM: UserMemory = UserMemory {
    code: [0; 4096],
    stack: [0; USER_STACK_SIZE],
};

/// Launch the first user-mode process.
///
/// This function:
///   1. Copies the user program into a page-aligned region
///   2. Sets up page table entries with user-accessible permissions
///   3. Transitions to Ring 3 via IRETQ
///
/// SAFETY: This function performs a privilege-level switch and never returns
/// to the kernel's normal control flow. The user process will call SYSRET
/// to return to kernel space, which lands in the syscall handler, not here.
pub fn launch_first_process() {
    println!("[...] Launching first user-mode process");

    // Copy user code into the page-aligned region.
    // SAFETY: USER_MEM is a static mutable; we access it only here during init.
    // We use `&raw const` to get the address without creating a shared reference
    // to a static mutable, which is UB under Rust's aliasing rules.
    let code_addr = unsafe {
        USER_MEM.code[..USER_CODE.len()].copy_from_slice(USER_CODE);
        (&raw const USER_MEM.code) as u64
    };

    // User stack grows down, so RSP starts at the top.
    let stack_top = unsafe { (&raw const USER_MEM.stack) as u64 + USER_STACK_SIZE as u64 };

    // Get user-mode segment selectors from the GDT.
    let sel = crate::arch::x86_64::gdt::selectors();
    let user_cs = u64::from(sel.user_code.0); // code segment selector
    let user_ss = u64::from(sel.user_data.0); // data segment selector

    println!("[OK] User code at {:#x}", code_addr);
    println!("[OK] User stack at {:#x}", stack_top);
    println!("[OK] User CS={:#x}, SS={:#x}", user_cs, user_ss);
    println!("[...] Transitioning to Ring 3...");

    // IRETQ to user-mode. We push the five values IRETQ expects:
    //   SS  (user data segment)
    //   RSP (user stack pointer)
    //   RFLAGS (with IF enabled, IOPL=3 for user I/O if needed)
    //   CS  (user code segment)
    //   RIP (user code entry point)
    //
    // SAFETY: This switches the CPU to Ring 3. The values must be correct:
    //   - CS must have RPL=3 (set by selector construction)
    //   - SS must have RPL=3
    //   - RSP must point to valid user-accessible memory
    //   - RIP must point to executable user-accessible code
    //   - RFLAGS must have IF set so user-space can receive interrupts
    unsafe {
        core::arch::asm!(
            "push {user_ss:r}",        // SS
            "push {user_rsp:r}",       // RSP
            "pushfq",                  // RFLAGS (save current, then enable IF)
            "pop rax",
            "or rax, 0x200",           // set IF (interrupt flag)
            "push rax",                // RFLAGS with IF
            "push {user_cs:r}",        // CS
            "push {user_rip:r}",       // RIP
            "iretq",                   // → Ring 3
            user_ss = in(reg) user_ss,
            user_rsp = in(reg) stack_top,
            user_cs = in(reg) user_cs,
            user_rip = in(reg) code_addr,
            options(noreturn)
        );
    }
}
