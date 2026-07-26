//! Unified page table abstraction for `x86_64` 4-level paging.
//!
//! Provides a single, safe interface for page table operations, replacing
//! the three separate walk implementations that previously existed in
//! `task/user.rs`, `memory/dma.rs`, and `syscall/mod.rs`.
//!
//! ## Design
//!
//! `PageTable` wraps a P4 physical address and provides methods to map,
//! unmap, and translate virtual addresses. All operations walk the 4-level
//! hierarchy (P4 → P3 → P2 → P1) and allocate intermediate tables as needed
//! via the global frame allocator.
//!
//! ## Safety
//!
//! The P4 physical address must point to a valid, aligned page table.
//! All page table entries are written through `phys_to_virt` which converts
//! physical addresses to kernel-accessible virtual addresses.

use x86_64::structures::paging::{PageTable as X86PageTable, PageTableFlags};

use crate::frame_alloc;
use crate::memory::phys_to_virt;

/// Number of entries in each page table level.
const PT_ENTRIES: usize = 512;

/// Page size (4 KiB).
pub const PAGE_SIZE: u64 = 0x1000;

/// User-space maximum virtual address (128 TiB, P4 index 256 boundary).
pub const USER_SPACE_MAX: u64 = 0x0000_8000_0000_0000;

/// Errors returned by page table operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageTableError {
    /// No physical frames available for intermediate tables.
    OutOfMemory,
    /// The virtual address is not mapped.
    NotMapped,
    /// The virtual address is already mapped.
    AlreadyMapped,
    /// The address is outside the valid range.
    InvalidAddress,
}

/// Check if every entry in a page table is non-present (all zeros).
fn table_is_empty(table: &X86PageTable) -> bool {
    table
        .iter()
        .all(|entry| !entry.flags().contains(PageTableFlags::PRESENT))
}

/// Free the frame backing `table`, clear the parent entry at `entry_idx`,
/// and invalidate the TLB for that entry's address range (not strictly
/// necessary for intermediate tables, but keeps the TLB consistent).
///
/// # Safety
///
/// `table` must point to a valid page table that was allocated by
/// `frame_alloc::alloc_frame()`. The `parent` table and `entry_idx` must
/// refer to the entry that points to `table`.
unsafe fn free_table_and_clear_entry(
    parent: &mut X86PageTable,
    entry_idx: usize,
    table_frame: u64,
) {
    parent[entry_idx].set_addr(x86_64::PhysAddr::new(0), PageTableFlags::empty());
    frame_alloc::free_frame(table_frame);
}

/// A handle to a 4-level `x86_64` page table.
///
/// Wraps the physical address of the P4 (level 4) table. All operations
/// walk the hierarchy from P4 down to P1.
pub struct PageTable {
    /// Physical address of the P4 table.
    p4_phys: u64,
}

impl PageTable {
    /// Create a new page table handle from a P4 physical address.
    ///
    /// # Safety
    ///
    /// `p4_phys` must be the physical address of a valid, aligned P4 page table.
    #[must_use]
    pub unsafe fn new(p4_phys: u64) -> Self {
        Self { p4_phys }
    }

    /// Get the physical address of the P4 table (CR3 value).
    #[must_use]
    pub fn cr3(&self) -> u64 {
        self.p4_phys
    }

    /// Get a mutable reference to the P4 table.
    ///
    /// # Safety
    ///
    /// The P4 table must be valid and not concurrently accessed.
    #[allow(clippy::mut_from_ref)]
    unsafe fn p4_mut(&self) -> &mut X86PageTable {
        // SAFETY: p4_phys is a valid page table address.
        unsafe { &mut *(phys_to_virt(self.p4_phys) as *mut X86PageTable) }
    }

    /// Map a single 4 KiB page.
    ///
    /// Walks P4 → P3 → P2 → P1, allocating intermediate tables as needed.
    /// Returns `Err(AlreadyMapped)` if the page is already present.
    ///
    /// # Arguments
    /// - `virt`: Virtual address (must be page-aligned).
    /// - `phys`: Physical address (must be page-aligned).
    /// - `flags`: Page table flags (must include PRESENT).
    ///
    /// # Errors
    ///
    /// Returns `AlreadyMapped` if the P1 entry is already present, or
    /// `OutOfMemory` if an intermediate table cannot be allocated.
    pub fn map_page(
        &self,
        virt: u64,
        phys: u64,
        flags: PageTableFlags,
    ) -> Result<(), PageTableError> {
        if virt % PAGE_SIZE != 0 || phys % PAGE_SIZE != 0 {
            return Err(PageTableError::InvalidAddress);
        }

        // SAFETY: We have exclusive access to the page table for this operation.
        let l4 = unsafe { self.p4_mut() };

        let p4_idx = ((virt >> 39) & 0x1FF) as usize;
        let p3_idx = ((virt >> 30) & 0x1FF) as usize;
        let p2_idx = ((virt >> 21) & 0x1FF) as usize;
        let p1_idx = ((virt >> 12) & 0x1FF) as usize;

        // Walk or create P3.
        let l3 = if l4[p4_idx].flags().contains(PageTableFlags::PRESENT) {
            unsafe {
                &mut *(phys_to_virt(l4[p4_idx].frame().unwrap().start_address().as_u64())
                    as *mut X86PageTable)
            }
        } else {
            let frame = frame_alloc::alloc_frame().ok_or(PageTableError::OutOfMemory)?;
            let table = unsafe { &mut *(phys_to_virt(frame) as *mut X86PageTable) };
            for entry in table.iter_mut() {
                entry.set_addr(x86_64::PhysAddr::new(0), PageTableFlags::empty());
            }
            l4[p4_idx].set_addr(
                x86_64::PhysAddr::new(frame),
                PageTableFlags::PRESENT
                    | PageTableFlags::WRITABLE
                    | PageTableFlags::USER_ACCESSIBLE,
            );
            table
        };

        // Walk or create P2.
        let l2 = if l3[p3_idx].flags().contains(PageTableFlags::PRESENT) {
            unsafe {
                &mut *(phys_to_virt(l3[p3_idx].frame().unwrap().start_address().as_u64())
                    as *mut X86PageTable)
            }
        } else {
            let frame = frame_alloc::alloc_frame().ok_or(PageTableError::OutOfMemory)?;
            let table = unsafe { &mut *(phys_to_virt(frame) as *mut X86PageTable) };
            for entry in table.iter_mut() {
                entry.set_addr(x86_64::PhysAddr::new(0), PageTableFlags::empty());
            }
            l3[p3_idx].set_addr(
                x86_64::PhysAddr::new(frame),
                PageTableFlags::PRESENT
                    | PageTableFlags::WRITABLE
                    | PageTableFlags::USER_ACCESSIBLE,
            );
            table
        };

        // Walk or create P1.
        let l1 = if l2[p2_idx].flags().contains(PageTableFlags::PRESENT) {
            let entry = &l2[p2_idx];
            if entry.flags().contains(PageTableFlags::HUGE_PAGE) {
                return Err(PageTableError::InvalidAddress);
            }
            unsafe {
                &mut *(phys_to_virt(entry.frame().unwrap().start_address().as_u64())
                    as *mut X86PageTable)
            }
        } else {
            let frame = frame_alloc::alloc_frame().ok_or(PageTableError::OutOfMemory)?;
            let table = unsafe { &mut *(phys_to_virt(frame) as *mut X86PageTable) };
            for entry in table.iter_mut() {
                entry.set_addr(x86_64::PhysAddr::new(0), PageTableFlags::empty());
            }
            l2[p2_idx].set_addr(
                x86_64::PhysAddr::new(frame),
                PageTableFlags::PRESENT
                    | PageTableFlags::WRITABLE
                    | PageTableFlags::USER_ACCESSIBLE,
            );
            table
        };

        // Check if already mapped.
        if l1[p1_idx].flags().contains(PageTableFlags::PRESENT) {
            return Err(PageTableError::AlreadyMapped);
        }

        // Set the P1 entry.
        l1[p1_idx].set_addr(x86_64::PhysAddr::new(phys), flags);
        Ok(())
    }

    /// Unmap a single 4 KiB page.
    ///
    /// Clears the P1 entry, invalidates the TLB entry, and frees empty
    /// intermediate page tables (P1, P2, P3) recursively. Returns the
    /// physical address that was mapped, or `Err(NotMapped)` if the page
    /// was not present.
    ///
    /// # Errors
    ///
    /// Returns `NotMapped` if the virtual address is not mapped.
    pub fn unmap_page(&self, virt: u64) -> Result<u64, PageTableError> {
        if virt % PAGE_SIZE != 0 {
            return Err(PageTableError::InvalidAddress);
        }

        // SAFETY: We have exclusive access to the page table.
        let l4 = unsafe { self.p4_mut() };

        let p4_idx = ((virt >> 39) & 0x1FF) as usize;
        let p3_idx = ((virt >> 30) & 0x1FF) as usize;
        let p2_idx = ((virt >> 21) & 0x1FF) as usize;
        let p1_idx = ((virt >> 12) & 0x1FF) as usize;

        if !l4[p4_idx].flags().contains(PageTableFlags::PRESENT) {
            return Err(PageTableError::NotMapped);
        }
        let l3 = unsafe {
            &mut *(phys_to_virt(l4[p4_idx].frame().unwrap().start_address().as_u64())
                as *mut X86PageTable)
        };

        if !l3[p3_idx].flags().contains(PageTableFlags::PRESENT) {
            return Err(PageTableError::NotMapped);
        }
        let l3_flags = l3[p3_idx].flags();
        if l3_flags.contains(PageTableFlags::HUGE_PAGE) {
            return Err(PageTableError::InvalidAddress);
        }
        let l3_frame = l3[p3_idx].frame().unwrap().start_address().as_u64();
        let l2 = unsafe { &mut *(phys_to_virt(l3_frame) as *mut X86PageTable) };

        if !l2[p2_idx].flags().contains(PageTableFlags::PRESENT) {
            return Err(PageTableError::NotMapped);
        }
        let l2_flags = l2[p2_idx].flags();
        if l2_flags.contains(PageTableFlags::HUGE_PAGE) {
            return Err(PageTableError::InvalidAddress);
        }
        let l2_frame = l2[p2_idx].frame().unwrap().start_address().as_u64();
        let l1 = unsafe { &mut *(phys_to_virt(l2_frame) as *mut X86PageTable) };

        if !l1[p1_idx].flags().contains(PageTableFlags::PRESENT) {
            return Err(PageTableError::NotMapped);
        }

        let phys = l1[p1_idx].addr().as_u64();
        l1[p1_idx].set_addr(x86_64::PhysAddr::new(0), PageTableFlags::empty());

        // Invalidate the TLB entry for this virtual address.
        // SAFETY: invlpg requires the address to be valid for TLB lookup;
        // any aligned virtual address is acceptable.
        unsafe {
            core::arch::asm!("invlpg [{}]", in(reg) virt);
        }

        // Free empty intermediate tables, walking back up: P1 → P2 → P3.
        if table_is_empty(l1) {
            // SAFETY: l2 and l2_frame are valid; we just cleared l1's entry.
            unsafe {
                free_table_and_clear_entry(l2, p2_idx, l2_frame);
            }
            // Check if P2 is now empty after clearing its entry.
            if table_is_empty(l2) {
                // SAFETY: l3 and l3_frame are valid; we just cleared l2's entry.
                unsafe {
                    free_table_and_clear_entry(l3, p3_idx, l3_frame);
                }
                // Check if P3 is now empty after clearing its entry.
                if table_is_empty(l3) {
                    let l4_frame = l4[p4_idx].frame().unwrap().start_address().as_u64();
                    // SAFETY: l4 is valid; we just cleared l3's entry.
                    unsafe {
                        free_table_and_clear_entry(l4, p4_idx, l4_frame);
                    }
                }
            }
        }

        Ok(phys)
    }

    /// Translate a virtual address to its physical address.
    ///
    /// Returns `Some(phys)` if the page is mapped, `None` otherwise.
    #[must_use]
    pub fn translate(&self, virt: u64) -> Option<u64> {
        // SAFETY: Read-only access to the page table.
        let l4 = unsafe { self.p4_mut() };

        let p4_idx = ((virt >> 39) & 0x1FF) as usize;
        let p3_idx = ((virt >> 30) & 0x1FF) as usize;
        let p2_idx = ((virt >> 21) & 0x1FF) as usize;
        let p1_idx = ((virt >> 12) & 0x1FF) as usize;

        if !l4[p4_idx].flags().contains(PageTableFlags::PRESENT) {
            return None;
        }
        let l3 = unsafe {
            &*(phys_to_virt(l4[p4_idx].frame().unwrap().start_address().as_u64())
                as *const X86PageTable)
        };

        if !l3[p3_idx].flags().contains(PageTableFlags::PRESENT) {
            return None;
        }
        if l3[p3_idx].flags().contains(PageTableFlags::HUGE_PAGE) {
            // 1 GiB huge page.
            let base = l3[p3_idx].addr().as_u64();
            return Some(base + (virt & 0x3FFF_FFFF));
        }
        let l2 = unsafe {
            &*(phys_to_virt(l3[p3_idx].frame().unwrap().start_address().as_u64())
                as *const X86PageTable)
        };

        if !l2[p2_idx].flags().contains(PageTableFlags::PRESENT) {
            return None;
        }
        if l2[p2_idx].flags().contains(PageTableFlags::HUGE_PAGE) {
            // 2 MiB huge page.
            let base = l2[p2_idx].addr().as_u64();
            return Some(base + (virt & 0x1F_FFFF));
        }
        let l1 = unsafe {
            &*(phys_to_virt(l2[p2_idx].frame().unwrap().start_address().as_u64())
                as *const X86PageTable)
        };

        if !l1[p1_idx].flags().contains(PageTableFlags::PRESENT) {
            return None;
        }

        let page_phys = l1[p1_idx].addr().as_u64();
        let offset = virt & 0xFFF; // Page offset.
        Some(page_phys + offset)
    }

    /// Find a free virtual address range of `count` contiguous pages.
    ///
    /// Starts scanning from `hint` (rounded up to page alignment) in the
    /// user-space half. Returns the first free virtual address, or `None`
    /// if no suitable range is found.
    ///
    /// # Arguments
    /// - `hint`: Preferred start address (page-aligned or rounded up).
    /// - `count`: Number of contiguous free pages needed.
    #[must_use]
    pub fn find_free_range(&self, hint: u64, count: usize) -> Option<u64> {
        let mut addr = (hint + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let mut run_start = addr;
        let mut run_len = 0;

        while addr < USER_SPACE_MAX {
            if self.translate(addr).is_some() {
                // Page is mapped — reset the run.
                run_len = 0;
                addr += PAGE_SIZE;
                run_start = addr;
            } else {
                if run_len == 0 {
                    run_start = addr;
                }
                run_len += 1;
                if run_len >= count {
                    return Some(run_start);
                }
                addr += PAGE_SIZE;
            }
        }
        None
    }

    /// Change the flags of an existing P1 entry.
    ///
    /// Returns `Err(NotMapped)` if the page is not present.
    ///
    /// # Errors
    ///
    /// Returns `NotMapped` if the virtual address is not mapped.
    pub fn protect_page(&self, virt: u64, flags: PageTableFlags) -> Result<(), PageTableError> {
        if virt % PAGE_SIZE != 0 {
            return Err(PageTableError::InvalidAddress);
        }

        // SAFETY: We have exclusive access to the page table.
        let l4 = unsafe { self.p4_mut() };

        let p4_idx = ((virt >> 39) & 0x1FF) as usize;
        let p3_idx = ((virt >> 30) & 0x1FF) as usize;
        let p2_idx = ((virt >> 21) & 0x1FF) as usize;
        let p1_idx = ((virt >> 12) & 0x1FF) as usize;

        if !l4[p4_idx].flags().contains(PageTableFlags::PRESENT) {
            return Err(PageTableError::NotMapped);
        }
        let l3 = unsafe {
            &mut *(phys_to_virt(l4[p4_idx].frame().unwrap().start_address().as_u64())
                as *mut X86PageTable)
        };
        if !l3[p3_idx].flags().contains(PageTableFlags::PRESENT) {
            return Err(PageTableError::NotMapped);
        }
        let l2 = unsafe {
            &mut *(phys_to_virt(l3[p3_idx].frame().unwrap().start_address().as_u64())
                as *mut X86PageTable)
        };
        if !l2[p2_idx].flags().contains(PageTableFlags::PRESENT) {
            return Err(PageTableError::NotMapped);
        }
        let l1 = unsafe {
            &mut *(phys_to_virt(l2[p2_idx].frame().unwrap().start_address().as_u64())
                as *mut X86PageTable)
        };
        if !l1[p1_idx].flags().contains(PageTableFlags::PRESENT) {
            return Err(PageTableError::NotMapped);
        }

        l1[p1_idx].set_flags(flags);

        // Invalidate the TLB entry so the new flags take effect immediately.
        // SAFETY: invlpg requires the address to be valid for TLB lookup;
        // any aligned virtual address is acceptable.
        unsafe {
            core::arch::asm!("invlpg [{}]", in(reg) virt);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_page_size() {
        assert_eq!(PAGE_SIZE, 0x1000);
    }

    #[test]
    fn test_page_table_error_inequality() {
        assert_eq!(PageTableError::OutOfMemory, PageTableError::OutOfMemory);
        assert_ne!(PageTableError::OutOfMemory, PageTableError::NotMapped);
    }

    #[test]
    fn test_page_table_cr3() {
        // SAFETY: Using a fake address for testing; we only call cr3().
        let pt = unsafe { PageTable::new(0x1000) };
        assert_eq!(pt.cr3(), 0x1000);
    }

    #[test]
    fn test_pt_entries() {
        assert_eq!(PT_ENTRIES, 512);
    }

    #[test]
    fn test_page_alignment_check() {
        // Unaligned addresses should return InvalidAddress.
        // SAFETY: Using a fake address; we only test the alignment check.
        let pt = unsafe { PageTable::new(0x1000) };
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
        assert_eq!(
            pt.map_page(0x1234, 0x5000, flags),
            Err(PageTableError::InvalidAddress)
        );
        assert_eq!(
            pt.map_page(0x1000, 0x5678, flags),
            Err(PageTableError::InvalidAddress)
        );
    }
}
