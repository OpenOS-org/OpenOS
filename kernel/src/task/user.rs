//! User-mode process management.
//!
//! Loads ELF executables from the initrd archive, maps their segments into
//! user-accessible pages, and transitions to Ring 3 via IRETQ.
//!
//! ## Flow
//!
//! 1. `kernel_main` extracts the ramdisk from `BootInfo` and passes it here
//! 2. We find the ELF binary in the initrd by name
//! 3. The ELF loader allocates physical frames, copies segments, maps pages
//! 4. IRETQ transitions to Ring 3 at the ELF entry point
//!
//! ## Page Table Strategy
//!
//! We reuse the kernel's P4 table and map user pages into the lower half
//! (virtual addresses < `0x0000_8000_0000_0000`). The `map_page` function
//! walks or creates P3/P2/P1 tables, patching existing entries with
//! `USER_ACCESSIBLE` where needed.

use x86_64::structures::paging::PageTableFlags;

use crate::serial_println;

/// User stack size: 2 pages = 8 KiB (set in elf.rs loader).
const USER_STACK_PAGES: u64 = 2;

/// Load an ELF executable from the initrd and launch it in Ring 3.
///
/// # Arguments
/// - `ramdisk`: raw bytes of the initrd archive
/// - `filename`: name of the ELF binary to load (e.g., "hello.elf")
///
/// # Panics
/// Panics if the initrd is invalid, the file is not found, or the ELF
/// cannot be loaded.
pub fn launch_from_initrd(ramdisk: &[u8], filename: &str, console_handle: u64) {
    crate::println!("[...] Loading '{filename}' from initrd");
    serial_println!("[...] Loading '{filename}' from initrd");

    // Find the ELF binary in the initrd.
    let file = crate::initrd::find_file(ramdisk, filename)
        .unwrap_or_else(|| panic!("'{filename}' not found in initrd"));

    crate::println!("[OK] Found '{filename}' ({} bytes)", file.data.len());
    serial_println!("[OK] Found '{filename}' ({} bytes)", file.data.len());

    // Load the ELF — allocates frames, copies segments, maps pages.
    let result = crate::elf::load_elf(file.data, |virt, phys, writable, executable| {
        let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
        if writable {
            flags |= PageTableFlags::WRITABLE;
        }
        if !executable {
            flags |= PageTableFlags::NO_EXECUTE;
        }
        unsafe {
            map_page(virt, phys, flags);
        }
    })
    .unwrap_or_else(|e| panic!("ELF load failed: {e:?}"));

    let user_rip = result.entry_point;
    let user_rsp = result.stack_top;

    crate::println!("[OK] ELF loaded: entry={user_rip:#x}, stack={user_rsp:#x}");
    crate::println!("[OK] Console handle: {console_handle:#x}");
    serial_println!("[OK] ELF loaded: entry={user_rip:#x}, stack={user_rsp:#x}");
    serial_println!("[OK] Console handle: {console_handle:#x}");

    // Transition to Ring 3.
    let sel = crate::arch::x86_64::gdt::selectors();
    let user_cs = u64::from(sel.user_code.0) | 3; // Ring 3
    let user_ss = u64::from(sel.user_data.0) | 3; // Ring 3

    serial_println!("[...] Transitioning to Ring 3 via IRETQ...");

    // SAFETY: All values are correct:
    //   - CS/SS have RPL=3 (Ring 3)
    //   - RSP points to the user stack (mapped with USER_ACCESSIBLE)
    //   - RIP points to the ELF entry point (mapped with USER_ACCESSIBLE, no NX)
    //   - RFLAGS has IF set (interrupts enabled in user-space)
    //   - RDI = console_handle (first argument to the entry point)
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
            "mov rdi, {handle:r}",  // Set RDI = handle AFTER pushing (RDI is scratch here)
            "iretq",
            user_ss = in(reg) user_ss,
            user_rsp = in(reg) user_rsp,
            user_cs = in(reg) user_cs,
            user_rip = in(reg) user_rip,
            handle = in(reg) console_handle,
            options(noreturn)
        );
    }
}

/// Map a single 4 KiB page in the active page tables.
///
/// Walks the page table hierarchy from P4 down to P1, creating missing
/// intermediate tables and patching existing entries with `USER_ACCESSIBLE`.
///
/// # Safety
/// - `virt` must be page-aligned.
/// - `phys` must be a valid, unused physical frame.
/// - `flags` must include `PRESENT` for the page to be accessible.
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

    // Ensure P4 entry has USER_ACCESSIBLE.
    if l4[p4_idx].flags().contains(PageTableFlags::PRESENT) {
        let old = l4[p4_idx].flags();
        if !old.contains(PageTableFlags::USER_ACCESSIBLE) {
            l4[p4_idx].set_flags(old | PageTableFlags::USER_ACCESSIBLE);
        }
    }

    // Walk or create P3.
    let l3 = if l4[p4_idx].flags().contains(PageTableFlags::PRESENT) {
        let l3 =
            &mut *(to_virt(l4[p4_idx].frame().unwrap().start_address().as_u64()) as *mut PageTable);
        if l3[p3_idx].flags().contains(PageTableFlags::PRESENT) {
            let old = l3[p3_idx].flags();
            if !old.contains(PageTableFlags::USER_ACCESSIBLE) {
                l3[p3_idx].set_flags(old | PageTableFlags::USER_ACCESSIBLE);
            }
        }
        l3
    } else {
        let frame = crate::frame_alloc::alloc_frame().expect("out of frames for P3 table");
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

    // Walk or create P2.
    let l2 = if l3[p3_idx].flags().contains(PageTableFlags::PRESENT) {
        &mut *(to_virt(l3[p3_idx].frame().unwrap().start_address().as_u64()) as *mut PageTable)
    } else {
        let frame = crate::frame_alloc::alloc_frame().expect("out of frames for P2 table");
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

    // Ensure P2 entry has USER_ACCESSIBLE.
    if l2[p2_idx].flags().contains(PageTableFlags::PRESENT) {
        let old = l2[p2_idx].flags();
        if !old.contains(PageTableFlags::USER_ACCESSIBLE) {
            l2[p2_idx].set_flags(old | PageTableFlags::USER_ACCESSIBLE);
        }
    }

    // Walk or create P1.
    let l1 = if l2[p2_idx].flags().contains(PageTableFlags::PRESENT) {
        let entry = &l2[p2_idx];
        assert!(
            !entry.flags().contains(PageTableFlags::HUGE_PAGE),
            "P2 entry is a huge page — use a different virtual address range"
        );
        &mut *(to_virt(entry.frame().unwrap().start_address().as_u64()) as *mut PageTable)
    } else {
        let frame = crate::frame_alloc::alloc_frame().expect("out of frames for P1 table");
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

/// Fallback: launch a hardcoded user program (no initrd).
///
/// Used when the bootloader does not provide a ramdisk. This is the same
/// "Hello from user-space!" program that was embedded as bytecode.
pub fn launch_first_process() {
    crate::println!("[SKIP] No initrd — cannot load user program");
    serial_println!("[SKIP] No initrd — cannot load user program");
}
