//! User-mode process management.
//!
//! This module handles the transition from kernel (Ring 0) to user-space (Ring 3).
//! The first user process is a hardcoded "hello world" program that demonstrates:
//!   1. Privilege-level transition via SYSRET (faster than IRETQ)
//!   2. System call via SYSCALL instruction
//!   3. Return to user-space via SYSRET
//!
//! ## Memory Layout
//!
//! The kernel's BSS section is mapped as 2 MiB huge pages by the bootloader.
//! We cannot set per-page flags (like clearing NX for code) on huge pages.
//! Instead, we create a separate P1 page table for user memory at a dedicated
//! virtual address range, with proper per-page permissions:
//!
//!   - Code page: `USER_ACCESSIBLE`, present, NOT writable, NOT NX (executable)
//!   - Stack page: `USER_ACCESSIBLE`, present, writable, NX (not executable)
//!
//! The user virtual addresses are chosen to be in a separate 2 MiB region
//! (`0x20_0000_0000`) that doesn't conflict with kernel mappings.

use x86_64::structures::paging::PageTableFlags;
use x86_64::VirtAddr;

use crate::serial_println;
use crate::syscall::number::{SYS_EXIT, SYS_WRITE};

/// User-mode stack: 1 page = 4 KiB.
const USER_STACK_SIZE: usize = 4096;

/// User virtual address base. Chosen to share P4[128]/P3[0] with the kernel
/// (which is at ~0x10000000000) but use a different P2 entry (P2[3] = 0x100000600000).
/// This avoids needing to create new P4/P3 tables — only a new P1 table is needed.
const USER_VIRT_BASE: u64 = 0x1000_0060_0000;

/// User code page offset from base.
const USER_CODE_OFFSET: u64 = 0x0000;
/// User stack page offset from base (next page after code).
const USER_STACK_OFFSET: u64 = 0x1000;

/// Physical frame for user pages. We allocate at 32 MiB, which is within
/// QEMU's default 128 MiB RAM and above the kernel (loaded at ~1 MiB).
const USER_PHYS_BASE: u64 = 0x0200_0000;

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
    // rip_after = 0x07, 0x07 + 0x49 = 0x50
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

/// Physical addresses for page table frames allocated for user memory.
/// These are at `USER_PHYS_BASE` + offset, in a region guaranteed free.
const PT_P3_FRAME: u64 = USER_PHYS_BASE + 0x3000;
const PT_P2_FRAME: u64 = USER_PHYS_BASE + 0x4000;
const PT_P1_FRAME: u64 = USER_PHYS_BASE + 0x5000;

/// Map a single 4 KiB page in the active page tables.
///
/// Walks the page table hierarchy from P4 down to P1, creating missing
/// intermediate tables at fixed physical frames. Uses the bootloader's
/// `physical_memory_offset` to access page table entries.
///
/// # Safety
/// - `virt` must be page-aligned.
/// - `phys` must be a valid, unused physical frame.
/// - `flags` must include PRESENT for the page to be accessible.
/// - The fixed PT frames at `USER_PHYS_BASE` + 0x3000..0x5000 must not be in use.
unsafe fn map_page(virt: u64, phys: u64, flags: PageTableFlags) {
    use x86_64::registers::control::Cr3;
    use x86_64::structures::paging::PageTable;

    let to_virt = crate::memory::phys_to_virt;

    let (level4_frame, _) = Cr3::read();
    let l4 = &mut *(to_virt(level4_frame.start_address().as_u64()) as *mut PageTable);

    let p4_idx = ((virt >> 39) & 0x1FF) as usize;
    let p3_idx = ((virt >> 30) & 0x1FF) as usize;
    let p2_idx = ((virt >> 21) & 0x1FF) as usize;
    let p1_idx = ((virt >> 12) & 0x1FF) as usize;

    // Ensure P4 entry has USER_ACCESSIBLE (the bootloader may not set it).
    if l4[p4_idx].flags().contains(PageTableFlags::PRESENT) {
        let old_flags = l4[p4_idx].flags();
        if !old_flags.contains(PageTableFlags::USER_ACCESSIBLE) {
            l4[p4_idx].set_flags(old_flags | PageTableFlags::USER_ACCESSIBLE);
        }
    }

    // Walk or create L3.
    let l3 = if l4[p4_idx].flags().contains(PageTableFlags::PRESENT) {
        let l3 =
            &mut *(to_virt(l4[p4_idx].frame().unwrap().start_address().as_u64()) as *mut PageTable);
        // Ensure P3 entry has USER_ACCESSIBLE.
        if l3[p3_idx].flags().contains(PageTableFlags::PRESENT) {
            let old = l3[p3_idx].flags();
            if !old.contains(PageTableFlags::USER_ACCESSIBLE) {
                l3[p3_idx].set_flags(old | PageTableFlags::USER_ACCESSIBLE);
            }
        }
        l3
    } else {
        let frame = PT_P3_FRAME;
        let table = &mut *(to_virt(frame) as *mut PageTable);
        for entry in table.iter_mut() {
            entry.set_addr(x86_64::PhysAddr::new(0), PageTableFlags::empty());
        }
        l4[p4_idx].set_addr(
            x86_64::PhysAddr::new(frame),
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE,
        );
        table
    };

    // Walk or create L2.
    let l2 = if l3[p3_idx].flags().contains(PageTableFlags::PRESENT) {
        &mut *(to_virt(l3[p3_idx].frame().unwrap().start_address().as_u64()) as *mut PageTable)
    } else {
        let frame = PT_P2_FRAME;
        let table = &mut *(to_virt(frame) as *mut PageTable);
        for entry in table.iter_mut() {
            entry.set_addr(x86_64::PhysAddr::new(0), PageTableFlags::empty());
        }
        l3[p3_idx].set_addr(
            x86_64::PhysAddr::new(frame),
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE,
        );
        table
    };

    // Ensure P2 entry has USER_ACCESSIBLE (existing kernel P2 entries may not have it).
    if l2[p2_idx].flags().contains(PageTableFlags::PRESENT) {
        let old = l2[p2_idx].flags();
        if !old.contains(PageTableFlags::USER_ACCESSIBLE) {
            l2[p2_idx].set_flags(old | PageTableFlags::USER_ACCESSIBLE);
        }
    }

    // Walk or create L1.
    let l1 = if l2[p2_idx].flags().contains(PageTableFlags::PRESENT) {
        let entry = &l2[p2_idx];
        assert!(
            !entry.flags().contains(PageTableFlags::HUGE_PAGE),
            "P2 entry is a huge page — use a different virtual address range"
        );
        &mut *(to_virt(entry.frame().unwrap().start_address().as_u64()) as *mut PageTable)
    } else {
        let frame = PT_P1_FRAME;
        let table = &mut *(to_virt(frame) as *mut PageTable);
        for entry in table.iter_mut() {
            entry.set_addr(x86_64::PhysAddr::new(0), PageTableFlags::empty());
        }
        l2[p2_idx].set_addr(
            x86_64::PhysAddr::new(frame),
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE,
        );
        table
    };

    // Set the P1 entry.
    l1[p1_idx].set_addr(x86_64::PhysAddr::new(phys), flags);
}

/// Launch the first user-mode process.
///
/// This function:
///   1. Copies the user program into a dedicated physical frame
///   2. Creates page table entries with proper per-page permissions
///   3. Transitions to Ring 3 via SYSRET
///
/// # Safety
/// This function transitions to Ring 3 and never returns. The user program
/// runs until it calls `SYS_EXIT`, which halts the CPU.
pub fn launch_first_process() {
    crate::println!("[...] Launching first user-mode process");
    serial_println!("[...] Launching first user-mode process");

    let code_virt = USER_VIRT_BASE + USER_CODE_OFFSET;
    let stack_virt = USER_VIRT_BASE + USER_STACK_OFFSET;
    let code_phys = USER_PHYS_BASE;
    let stack_phys = USER_PHYS_BASE + 0x1000;

    // Copy user code to the physical frame.
    // SAFETY: USER_PHYS_BASE is a high physical address (32 MiB) that is
    // guaranteed to be free (kernel loads below 16 MiB). We use phys_to_virt
    // to write to it through the physical memory mapping.
    let code_dest = crate::memory::phys_to_virt(code_phys) as *mut u8;
    unsafe {
        core::ptr::write_bytes(code_dest, 0, 4096); // zero the page
        core::ptr::copy_nonoverlapping(USER_CODE.as_ptr(), code_dest, USER_CODE.len());
    }

    // Zero the stack page.
    let stack_dest = crate::memory::phys_to_virt(stack_phys) as *mut u8;
    unsafe {
        core::ptr::write_bytes(stack_dest, 0, 4096);
    }

    // Map code page: present, user-accessible, NOT writable, NOT NX (executable).
    let code_flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    // Map stack page: present, user-accessible, writable, NX (not executable).
    let stack_flags =
        PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE | PageTableFlags::WRITABLE;

    // SAFETY: We're creating new page table entries for dedicated physical frames.
    // The virtual addresses don't conflict with kernel mappings.
    unsafe {
        map_page(code_virt, code_phys, code_flags);
        map_page(stack_virt, stack_phys, stack_flags);
    }

    crate::println!("[OK] User pages mapped at {:#x}", USER_VIRT_BASE);
    serial_println!("[OK] User pages mapped at {:#x}", USER_VIRT_BASE);

    // User stack grows downward, so RSP points to the top of the stack page.
    let user_rsp = stack_virt + USER_STACK_SIZE as u64;
    let user_rip = code_virt;

    // Get segment selectors for the SYSRET transition.
    let sel = crate::arch::x86_64::gdt::selectors();
    let user_cs = sel.user_code.0;
    let user_ss = sel.user_data.0;

    crate::println!("[OK] User RIP={:#x}, RSP={:#x}", user_rip, user_rsp);
    crate::println!("[OK] User CS={:#x}, SS={:#x}", user_cs, user_ss);
    serial_println!("[OK] User RIP={:#x}, RSP={:#x}", user_rip, user_rsp);
    serial_println!("[...] Transitioning to Ring 3 via IRETQ...");

    // IRETQ to Ring 3.
    //
    // IRETQ pops SS, RSP, RFLAGS, CS, RIP from the stack and transitions
    // to the specified privilege level. This is more explicit than SYSRET
    // and avoids the STAR MSR dependency for the initial transition.
    //
    // SAFETY: All values are correct:
    //   - CS/SS have RPL=3 (Ring 3)
    //   - RSP points to the user stack (mapped with USER_ACCESSIBLE)
    //   - RIP points to user code (mapped with USER_ACCESSIBLE, no NX)
    //   - RFLAGS has IF set (interrupts enabled in user-space)
    let user_cs = u64::from(user_cs) | 3; // Ring 3
    let user_ss = u64::from(user_ss) | 3; // Ring 3
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
            user_rsp = in(reg) user_rsp,
            user_cs = in(reg) user_cs,
            user_rip = in(reg) user_rip,
            options(noreturn)
        );
    }
}
