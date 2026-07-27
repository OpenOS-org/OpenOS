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

    // Create a per-process page table for the first process.
    // This provides address space isolation from the kernel.
    let page_table_phys = crate::memory::create_user_page_table()
        .expect("out of memory for first process page table");

    // Switch to the new page table so map_page writes into it.
    let (kernel_p4, _) = x86_64::registers::control::Cr3::read();
    let kernel_cr3 = kernel_p4.start_address().as_u64();
    // SAFETY: page_table_phys was just allocated and is valid.
    unsafe {
        crate::memory::switch_page_table(page_table_phys);
    }

    // Load the ELF — allocates frames, copies segments, maps pages.
    let result = crate::elf::load_elf(file.data, |virt, phys, writable, executable| {
        let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
        if writable {
            flags |= PageTableFlags::WRITABLE;
        }
        if !executable {
            flags |= PageTableFlags::NO_EXECUTE;
        }
        // SAFETY: `virt` is page-aligned, `phys` was allocated by the ELF loader's
        // frame allocator, and `flags` correctly reflect the segment permissions.
        unsafe {
            map_page(virt, phys, flags);
        }
    })
    .unwrap_or_else(|e| panic!("ELF load failed: {e:?}"));

    // Switch back to kernel page table.
    // SAFETY: kernel_cr3 is the original bootloader page table.
    unsafe {
        crate::memory::switch_page_table(kernel_cr3);
    }

    let user_rip = result.entry_point;
    let user_rsp = result.stack_top;

    crate::println!("[OK] ELF loaded: entry={user_rip:#x}, stack={user_rsp:#x}");
    crate::println!("[OK] Console handle: {console_handle:#x}");
    serial_println!("[OK] ELF loaded: entry={user_rip:#x}, stack={user_rsp:#x}");
    serial_println!("[OK] Console handle: {console_handle:#x}");

    // Register this process in the scheduler so it gets a proper Task with
    // page_table set, enabling CR3 switching on context switch.
    let first_task = crate::task::task::Task::new(filename, 10);
    let first_task_id = first_task.id;
    crate::task::scheduler::with_task_mut(first_task_id, |task| {
        task.page_table = Some(page_table_phys);
        task.context = Some(crate::task::task::SavedContext::user_mode(
            user_rip,
            user_rsp,
            page_table_phys,
        ));
    });
    crate::task::scheduler::set_current_task(first_task_id);

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
            "mov cr3, {cr3:r}",     // Load the process's page table
            "iretq",
            user_ss = in(reg) user_ss,
            user_rsp = in(reg) user_rsp,
            user_cs = in(reg) user_cs,
            user_rip = in(reg) user_rip,
            handle = in(reg) console_handle,
            cr3 = in(reg) page_table_phys,
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
///
/// This is the public interface used by `sys_process_start` to map pages
/// into a user process's page table while it is loaded in CR3.
pub unsafe fn map_page_user(virt: u64, phys: u64, flags: PageTableFlags) {
    // SAFETY: Caller guarantees validity of virt, phys, and flags.
    unsafe {
        map_page(virt, phys, flags);
    }
}

/// Change protection flags on a single page in the active page tables.
///
/// # Safety
/// - `virt` must be page-aligned and currently mapped.
/// - `flags` must include `PRESENT` for the page to remain accessible.
pub unsafe fn protect_page_user(virt: u64, flags: PageTableFlags) {
    use x86_64::registers::control::Cr3;
    use x86_64::structures::paging::PageTable;

    let to_virt = crate::memory::phys_to_virt;

    let (level4_frame, _) = Cr3::read();
    let l4 = &mut *(to_virt(level4_frame.start_address().as_u64()) as *mut PageTable);

    let p4_idx = ((virt >> 39) & 0x1FF) as usize;
    let p3_idx = ((virt >> 30) & 0x1FF) as usize;
    let p2_idx = ((virt >> 21) & 0x1FF) as usize;
    let p1_idx = ((virt >> 12) & 0x1FF) as usize;

    if !l4[p4_idx].flags().contains(PageTableFlags::PRESENT) {
        return;
    }
    let l3 = &mut *(to_virt(l4[p4_idx].addr().as_u64()) as *mut PageTable);
    if !l3[p3_idx].flags().contains(PageTableFlags::PRESENT) {
        return;
    }

    // Handle 1 GiB huge page.
    if l3[p3_idx].flags().contains(PageTableFlags::HUGE_PAGE) {
        // Cannot protect individual pages within a huge page.
        return;
    }

    let l2 = &mut *(to_virt(l3[p3_idx].addr().as_u64()) as *mut PageTable);
    if !l2[p2_idx].flags().contains(PageTableFlags::PRESENT) {
        return;
    }

    // Handle 2 MiB huge page.
    if l2[p2_idx].flags().contains(PageTableFlags::HUGE_PAGE) {
        return;
    }

    let l1 = &mut *(to_virt(l2[p2_idx].addr().as_u64()) as *mut PageTable);
    if !l1[p1_idx].flags().contains(PageTableFlags::PRESENT) {
        return;
    }

    l1[p1_idx].set_flags(flags);

    // Invalidate TLB entry.
    core::arch::asm!("invlpg [{}]", in(reg) virt);
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
        if entry.flags().contains(PageTableFlags::HUGE_PAGE) {
            // Split the 2 MiB huge page into 512 × 4 KiB P1 entries so we
            // can map a single page at a specific virtual address within it.
            serial_println!("[DEBUG] Splitting 2 MiB huge page at P2[{p2_idx}] for virt={virt:#x}");
            split_huge_page(l2, p2_idx, to_virt)
        } else {
            &mut *(to_virt(entry.frame().unwrap().start_address().as_u64()) as *mut PageTable)
        }
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

/// Split a 2 MiB huge page at P2 into 512 individual 4 KiB P1 entries.
///
/// This is needed when the kernel wants to map a single 4 KiB page inside a
/// 2 MiB region that was previously mapped as a huge page (e.g., by the
/// bootloader's physical memory mapping).
///
/// The original physical frame backing the huge page is NOT freed — it may
/// still be in use by the kernel. Instead, we create a new P1 table whose
/// entries each point to the corresponding 4 KiB sub-page of the 2 MiB region.
///
/// # Safety
///
/// `l2` must point to a valid P2 page table. `p2_idx` must be 0..512 and
/// the entry must be a 2 MiB huge page. `to_virt` must convert physical
/// addresses to valid kernel virtual addresses.
unsafe fn split_huge_page(
    l2: &mut x86_64::structures::paging::PageTable,
    p2_idx: usize,
    to_virt: fn(u64) -> u64,
) -> &mut x86_64::structures::paging::PageTable {
    let entry = &l2[p2_idx];
    let huge_phys = entry.addr().as_u64();
    let huge_flags = entry.flags();

    // Allocate a new P1 table frame.
    let p1_frame =
        crate::frame_alloc::alloc_frame().expect("out of frames for P1 table (huge page split)");
    let p1 = &mut *(to_virt(p1_frame) as *mut x86_64::structures::paging::PageTable);

    // Fill all 512 entries pointing into the 2 MiB region.
    for i in 0..512 {
        let page_phys = huge_phys + (i as u64) * 0x1000;
        let mut page_flags = huge_flags;
        page_flags.remove(PageTableFlags::HUGE_PAGE);
        p1[i].set_addr(x86_64::PhysAddr::new(page_phys), page_flags);
    }

    // Update the P2 entry to point to the new P1 table.
    let mut new_flags = huge_flags;
    new_flags.remove(PageTableFlags::HUGE_PAGE);
    l2[p2_idx].set_addr(
        x86_64::PhysAddr::new(p1_frame),
        new_flags | PageTableFlags::WRITABLE,
    );

    p1
}

/// Fallback: launch a hardcoded user program (no initrd).
///
/// Used when the bootloader does not provide a ramdisk. This is the same
/// "Hello from user-space!" program that was embedded as bytecode.
pub fn launch_first_process() {
    crate::println!("[SKIP] No initrd — cannot load user program");
    serial_println!("[SKIP] No initrd — cannot load user program");
}

/// Free all user-space pages in a page table and the page table itself.
///
/// Walks the lower half of the P4 table (indices 0..256) and frees all
/// mapped physical frames at P1, P2, and P3 levels, then frees the P4
/// table frame itself.
///
/// # Safety
///
/// `p4_phys` must be the physical address of a page table that was
/// created by `create_user_page_table()` and is no longer in use (not
/// loaded in CR3).
///
/// # Panics
///
/// Panics if `phys_to_virt` is called before `set_physical_memory_offset`.
pub unsafe fn free_user_page_table(p4_phys: u64) {
    use x86_64::structures::paging::PageTable;

    let to_virt = crate::memory::phys_to_virt;

    // SAFETY: `p4_phys` is a valid page table not currently in use.
    let l4 = unsafe { &mut *(to_virt(p4_phys) as *mut PageTable) };

    // Walk lower half only (user space: indices 0..256).
    for p4_idx in 0..256 {
        if !l4[p4_idx].flags().contains(PageTableFlags::PRESENT) {
            continue;
        }
        let l3_phys = l4[p4_idx].frame().unwrap().start_address().as_u64();
        // SAFETY: P4 entry is PRESENT, so this frame is a valid P3 table.
        let l3 = unsafe { &mut *(to_virt(l3_phys) as *mut PageTable) };

        for p3_idx in 0..512 {
            if !l3[p3_idx].flags().contains(PageTableFlags::PRESENT) {
                continue;
            }

            let l3_flags = l3[p3_idx].flags();
            if l3_flags.contains(PageTableFlags::HUGE_PAGE) {
                // 1 GiB huge page — free it (unusual in user space).
                crate::frame_alloc::free_frame(l3[p3_idx].addr().as_u64());
                continue;
            }

            let l2_phys = l3[p3_idx].frame().unwrap().start_address().as_u64();
            // SAFETY: P3 entry is PRESENT and not a huge page, so this is a valid P2 table.
            let l2 = unsafe { &mut *(to_virt(l2_phys) as *mut PageTable) };

            for p2_idx in 0..512 {
                if !l2[p2_idx].flags().contains(PageTableFlags::PRESENT) {
                    continue;
                }

                let l2_flags = l2[p2_idx].flags();
                if l2_flags.contains(PageTableFlags::HUGE_PAGE) {
                    // 2 MiB huge page — free it.
                    crate::frame_alloc::free_frame(l2[p2_idx].addr().as_u64());
                    continue;
                }

                let l1_phys = l2[p2_idx].frame().unwrap().start_address().as_u64();
                // SAFETY: P2 entry is PRESENT and not a huge page, so this is a valid P1 table.
                let l1 = unsafe { &mut *(to_virt(l1_phys) as *mut PageTable) };

                // Free all mapped P1 entries (user pages).
                for p1_idx in 0..512 {
                    if l1[p1_idx].flags().contains(PageTableFlags::PRESENT) {
                        crate::frame_alloc::free_frame(l1[p1_idx].addr().as_u64());
                    }
                }

                // Free the P1 table frame itself.
                crate::frame_alloc::free_frame(l1_phys);
            }

            // Free the P2 table frame itself.
            crate::frame_alloc::free_frame(l2_phys);
        }

        // Free the P3 table frame itself.
        crate::frame_alloc::free_frame(l3_phys);
    }

    // Free the P4 table frame itself.
    crate::frame_alloc::free_frame(p4_phys);

    serial_println!("[MEM] Freed user page table at {:#x}", p4_phys);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_stack_pages() {
        assert_eq!(USER_STACK_PAGES, 2);
    }

    #[test]
    fn test_page_flags_code() {
        // Code page: PRESENT + USER_ACCESSIBLE, no WRITABLE, no NX.
        let flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
        assert!(flags.contains(PageTableFlags::PRESENT));
        assert!(flags.contains(PageTableFlags::USER_ACCESSIBLE));
        assert!(!flags.contains(PageTableFlags::WRITABLE));
        assert!(!flags.contains(PageTableFlags::NO_EXECUTE));
    }

    #[test]
    fn test_page_flags_data() {
        // Data page: PRESENT + USER_ACCESSIBLE + WRITABLE + NO_EXECUTE.
        let flags = PageTableFlags::PRESENT
            | PageTableFlags::USER_ACCESSIBLE
            | PageTableFlags::WRITABLE
            | PageTableFlags::NO_EXECUTE;
        assert!(flags.contains(PageTableFlags::WRITABLE));
        assert!(flags.contains(PageTableFlags::NO_EXECUTE));
    }

    #[test]
    fn test_page_flags_stack() {
        // Stack page: same as data page.
        let flags =
            PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE | PageTableFlags::WRITABLE;
        assert!(flags.contains(PageTableFlags::PRESENT));
        assert!(flags.contains(PageTableFlags::WRITABLE));
        assert!(flags.contains(PageTableFlags::USER_ACCESSIBLE));
    }
}
