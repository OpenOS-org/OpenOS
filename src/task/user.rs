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

use x86_64::structures::paging::PageTableFlags;
use x86_64::VirtAddr;

use crate::println;
use crate::syscall::number::{SYS_EXIT, SYS_WRITE};

/// User-mode stack: 1 page = 4 KiB.
const USER_STACK_SIZE: usize = 4096;

/// User-mode program: calls `SYS_WRITE` then `SYS_EXIT`.
///
/// Syscall convention (see `syscall::number`):
///   RAX = number, RDI = arg1, RSI = arg2, RDX = arg3
///
/// Layout:
///   [0x00..0x07] lea rdi, [rip + 0x49]   → string at 0x50 (buf pointer in RDI)
///   [0x07..0x0e] mov rsi, 23             → length in RSI
///   [0x0e..0x15] mov rax, `SYS_WRITE` (1)
///   [0x15..0x17] syscall
///   [0x17..0x1e] mov rdi, 0              → exit code 0 (arg1 for `SYS_EXIT`)
///   [0x1e..0x25] mov rax, `SYS_EXIT` (3)
///   [0x25..0x27] syscall
///   [0x27..0x50] padding
///   [0x50..0x67] "Hello from user-space!\n" (23 bytes)
const USER_CODE: [u8; 0x67] = [
    // lea rdi, [rip + 0x49]  → string at offset 0x50 (arg1 = buffer pointer)
    // rip = 0x07 (end of this instruction), 0x07 + 0x49 = 0x50
    0x48, 0x8d, 0x3d, 0x49, 0x00, 0x00, 0x00, // [0x00..0x07]
    // mov rsi, 23 (0x17)  → "Hello from user-space!\n" length (arg2 = length)
    0x48, 0xc7, 0xc6, 0x17, 0x00, 0x00, 0x00, // [0x07..0x0e]
    // mov rax, SYS_WRITE (1)
    0x48, 0xc7, 0xc0, 0x01, 0x00, 0x00, 0x00, // [0x0e..0x15]
    // syscall
    0x0f, 0x05, // [0x15..0x17]
    // mov rdi, 0  → exit code 0 (arg1 for SYS_EXIT)
    0x48, 0xc7, 0xc7, 0x00, 0x00, 0x00, 0x00, // [0x17..0x1e]
    // mov rax, SYS_EXIT (3)
    0x48, 0xc7, 0xc0, 0x03, 0x00, 0x00, 0x00, // [0x1e..0x25]
    // syscall
    0x0f, 0x05, // [0x25..0x27]
    // jmp $ (infinite loop if exit returns)
    0xeb, 0xfe, // [0x27..0x29]
    // Padding to offset 0x50 (39 bytes)
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // [0x29..0x31]
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // [0x31..0x39]
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // [0x39..0x41]
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // [0x41..0x49]
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // [0x49..0x50]
    // "Hello from user-space!\n" at offset 0x50 (23 bytes)
    b'H', b'e', b'l', b'l', b'o', b' ', b'f', b'r', // [0x50..0x58]
    b'o', b'm', b' ', b'u', b's', b'e', b'r', b'-', // [0x58..0x60]
    b's', b'p', b'a', b'c', b'e', b'!', b'\n', // [0x60..0x67]
];

/// User-mode static data. Page-aligned so we can set per-page permissions.
///
/// The bootloader identity-maps the first 2 MiB with supervisor-only
/// permissions. We must modify the page table to set the USER bit on
/// the pages containing user code and stack, otherwise Ring 3 access
/// triggers a page fault.
#[repr(align(4096))]
struct UserMemory {
    code: [u8; 4096],
    stack: [u8; USER_STACK_SIZE],
}

static mut USER_MEM: UserMemory = UserMemory {
    code: [0; 4096],
    stack: [0; USER_STACK_SIZE],
};

/// Set the `USER_ACCESSIBLE` flag on a page table entry for a given virtual address.
///
/// The bootloader's identity mapping uses supervisor-only pages. Ring 3 code
/// will #PF on any page without the USER bit (bit 2) set. This function walks
/// the active page table and sets the bit.
///
/// SAFETY: `addr` must be page-aligned and mapped by the bootloader's identity
/// mapping. We only ADD the USER flag, which doesn't affect Ring 0 access.
unsafe fn set_user_accessible(addr: VirtAddr) {
    use x86_64::registers::control::Cr3;
    use x86_64::structures::paging::PageTable;

    let (level4_frame, _) = Cr3::read();
    let l4_phys = level4_frame.start_address().as_u64();

    // SAFETY: The bootloader's page tables are identity-mapped, so physical
    // addresses equal virtual addresses for the first 2 MiB. We cast the
    // physical address directly to a pointer.
    let l4 = &mut *(l4_phys as *mut PageTable);

    let l3_phys = l4[addr.p4_index()]
        .frame()
        .unwrap()
        .start_address()
        .as_u64();
    let l3 = &mut *(l3_phys as *mut PageTable);

    let l2_phys = l3[addr.p3_index()]
        .frame()
        .unwrap()
        .start_address()
        .as_u64();
    let l2 = &mut *(l2_phys as *mut PageTable);

    let l1_phys = l2[addr.p2_index()]
        .frame()
        .unwrap()
        .start_address()
        .as_u64();
    let l1 = &mut *(l1_phys as *mut PageTable);

    let entry = &mut l1[addr.p1_index()];
    let flags = entry.flags();
    entry.set_flags(flags | PageTableFlags::USER_ACCESSIBLE);
}

/// Launch the first user-mode process.
///
/// Steps:
///   1. Copy machine code into page-aligned memory
///   2. Set `USER_ACCESSIBLE` bit on code and stack pages
///   3. IRETQ to Ring 3
pub fn launch_first_process() {
    println!("[...] Launching first user-mode process");

    let code_addr = unsafe {
        USER_MEM.code[..USER_CODE.len()].copy_from_slice(&USER_CODE);
        (&raw const USER_MEM.code) as u64
    };
    let stack_top = unsafe { (&raw const USER_MEM.stack) as u64 + USER_STACK_SIZE as u64 };

    // Set USER_ACCESSIBLE on the pages containing user code and stack.
    // Without this, Ring 3 access triggers #PF (page fault, error code bit 2 = user-mode).
    //
    // SAFETY: We're setting the USER bit on identity-mapped pages. This is safe
    // because the pages are already mapped — we're only relaxing permissions.
    // Ring 0 code can still access these pages (supervisor can access user pages).
    unsafe {
        set_user_accessible(VirtAddr::new(code_addr));
        set_user_accessible(VirtAddr::new(stack_top - 1)); // stack_top is in the last page
    }
    println!("[OK] Page tables updated (USER_ACCESSIBLE set)");

    let sel = crate::arch::x86_64::gdt::selectors();
    let user_cs = u64::from(sel.user_code.0);
    let user_ss = u64::from(sel.user_data.0);

    println!("[OK] User code at {:#x}", code_addr);
    println!("[OK] User stack at {:#x}", stack_top);
    println!("[OK] User CS={:#x}, SS={:#x}", user_cs, user_ss);
    println!("[...] Transitioning to Ring 3 via IRETQ...");

    // IRETQ to Ring 3.
    //
    // SAFETY: The IRETQ instruction pops SS, RSP, RFLAGS, CS, RIP from the stack
    // and transitions to the specified privilege level. All values are correct:
    //   - CS/SS have RPL=3 (Ring 3)
    //   - RSP points to the user stack (now USER_ACCESSIBLE)
    //   - RIP points to user code (now USER_ACCESSIBLE)
    //   - RFLAGS has IF set (interrupts enabled in user-space)
    unsafe {
        core::arch::asm!(
            "push {user_ss:r}",
            "push {user_rsp:r}",
            "pushfq",
            "pop rax",
            "or rax, 0x200",    // IF = 1
            "push rax",
            "push {user_cs:r}",
            "push {user_rip:r}",
            "iretq",
            user_ss = in(reg) user_ss,
            user_rsp = in(reg) stack_top,
            user_cs = in(reg) user_cs,
            user_rip = in(reg) code_addr,
            options(noreturn)
        );
    }
}
