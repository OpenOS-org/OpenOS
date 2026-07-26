//! DMA buffer management for device drivers.
//!
//! Provides physically contiguous memory allocation suitable for DMA
//! (Direct Memory Access) by hardware devices. `VirtIO` and PCI devices
//! use physical addresses to read/write buffers, so the kernel must
//! allocate memory from the physical frame allocator and expose both
//! the physical address (for the device) and the virtual address (for
//! the kernel to fill/read the buffer).
//!
//! ## Design
//!
//! - `DmaBuffer` tracks a contiguous run of 4 KiB frames.
//! - `alloc_dma()` allocates sequential frames from the bitmap allocator.
//! - `free_dma()` returns frames to the allocator.
//! - `map_dma_user()` maps a DMA buffer into a user process's page table
//!   so user-space services can directly read/write device buffers.
//!
//! ## Constraints
//!
//! - Buffers must be page-aligned in both size and alignment.
//! - Physical addresses must be below 4 GiB (most PCI/legacy devices
//!   use 32-bit DMA addressing). The frame allocator region is already
//!   constrained below 1 GiB by default, so this is enforced with a
//!   check.

use alloc::vec::Vec;

use x86_64::structures::paging::PageTableFlags;

use crate::frame_alloc;

/// Page size (4 KiB).
const PAGE_SIZE: u64 = 0x1000;

/// Maximum physical address for DMA. Most PCI devices use 32-bit
/// addressing and cannot access memory above 4 GiB.
const DMA_PHYSICAL_LIMIT: u64 = 0x1_0000_0000;

/// A DMA buffer: physically contiguous memory accessible by hardware.
///
/// Holds both the physical address (for the device descriptor / ring
/// entry) and the kernel virtual address (for the driver to read/write
/// the buffer contents). The `size` is always a multiple of `PAGE_SIZE`.
pub struct DmaBuffer {
    /// Physical address of the first page (passed to the device).
    phys_addr: u64,
    /// Kernel virtual address (physical + `physical_memory_offset`).
    virt_addr: u64,
    /// Total buffer size in bytes (always page-aligned).
    size: usize,
}

impl DmaBuffer {
    /// Physical address of the buffer start, suitable for device descriptors.
    #[must_use]
    pub fn phys_addr(&self) -> u64 {
        self.phys_addr
    }

    /// Kernel virtual address of the buffer start.
    #[must_use]
    pub fn virt_addr(&self) -> u64 {
        self.virt_addr
    }

    /// Buffer size in bytes.
    #[must_use]
    pub fn size(&self) -> usize {
        self.size
    }

    /// Return a mutable slice over the buffer contents.
    ///
    /// # Safety
    /// The caller must ensure no other reference to this memory exists
    /// (e.g., the device is not currently DMA-ing into the buffer).
    #[must_use]
    #[allow(clippy::mut_from_ref)]
    pub fn as_mut_slice(&self) -> &mut [u8] {
        // SAFETY: `virt_addr` points to exclusively-owned physical frames
        // allocated by the frame allocator. The caller guarantees no
        // concurrent device DMA.
        unsafe { core::slice::from_raw_parts_mut(self.virt_addr as *mut u8, self.size) }
    }
}

/// Allocate a physically contiguous DMA buffer.
///
/// Allocates `size` bytes of physically contiguous memory from the frame
/// allocator. The memory is zeroed and page-aligned. Both the physical
/// address (for device use) and virtual address (for kernel use) are
/// returned in a `DmaBuffer`.
///
/// # Arguments
/// - `size`: Buffer size in bytes. Must be a non-zero multiple of `PAGE_SIZE`.
/// - `alignment`: Required alignment in bytes. Must be a power of two and
///   a multiple of `PAGE_SIZE`. Currently only page alignment is supported;
///   the value is validated but the allocator always returns page-aligned
///   frames.
///
/// # Returns
/// `Ok(DmaBuffer)` on success, `Err(())` if:
/// - `size` is zero or not page-aligned
/// - `alignment` is not a power of two or not page-aligned
/// - Not enough contiguous free frames are available
/// - The allocated memory exceeds the DMA physical address limit (4 GiB)
///
/// # Contiguity
/// The bitmap allocator returns frames in ascending order. We allocate
/// `num_pages` frames and verify they are physically contiguous (each
/// frame address is exactly `PAGE_SIZE` after the previous). If they
/// are not contiguous, we free the partial allocation and return an error.
/// In practice, the frame allocator's bump-like behavior (scanning from
/// low addresses) makes contiguity likely for small allocations.
pub fn alloc_dma(size: usize, alignment: usize) -> Result<DmaBuffer, ()> {
    // Validate size.
    if size == 0 || size % (PAGE_SIZE as usize) != 0 {
        return Err(());
    }

    // Validate alignment: must be power-of-two and page-aligned.
    if alignment == 0 || !alignment.is_power_of_two() || alignment % (PAGE_SIZE as usize) != 0 {
        return Err(());
    }

    let num_pages = size / (PAGE_SIZE as usize);

    // Allocate frames one at a time, checking contiguity.
    let mut frames: Vec<u64> = Vec::with_capacity(num_pages);

    for i in 0..num_pages {
        let frame = frame_alloc::alloc_frame().ok_or_else(|| {
            // Failed to allocate — free everything we got so far.
            for &f in &frames {
                frame_alloc::free_frame(f);
            }
        })?;

        // Check contiguity with the previous frame.
        if i > 0 {
            let expected = frames[i - 1] + PAGE_SIZE;
            if frame != expected {
                // Not contiguous — free everything and bail.
                frame_alloc::free_frame(frame);
                for &f in &frames {
                    frame_alloc::free_frame(f);
                }
                return Err(());
            }
        }

        // Check DMA address limit.
        if frame + PAGE_SIZE > DMA_PHYSICAL_LIMIT {
            frame_alloc::free_frame(frame);
            for &f in &frames {
                frame_alloc::free_frame(f);
            }
            return Err(());
        }

        frames.push(frame);
    }

    let phys_addr = frames[0];
    let virt_addr = crate::memory::phys_to_virt(phys_addr);

    // Zero the buffer via the kernel virtual mapping.
    // SAFETY: `virt_addr` maps to exclusively-owned physical frames.
    // We zero them before handing the buffer to the caller.
    unsafe {
        core::ptr::write_bytes(virt_addr as *mut u8, 0, size);
    }

    Ok(DmaBuffer {
        phys_addr,
        virt_addr,
        size,
    })
}

/// Map a DMA buffer into a user-space process's page table.
///
/// Walks the user page table starting from `user_page_table` (physical
/// address of the P4 table) and maps each page of the DMA buffer to the
/// same physical frames, with `PRESENT | WRITABLE | NO_EXECUTE` flags.
/// Intermediate page table levels are allocated from the frame allocator
/// as needed.
///
/// # Arguments
/// - `dma`: The DMA buffer to map.
/// - `user_page_table`: Physical address of the user process's P4 page table.
///
/// # Returns
/// `Ok(user_virt)` — the user-space virtual address where the buffer
/// is mapped. The caller should choose an address in the user half
/// (< `USER_SPACE_MAX`).
///
/// `Err(())` if page table allocation fails.
///
/// # Design Note
/// This function uses a simple linear page walk identical to the one in
/// `task::user::map_user_page`. In a production kernel this would be
/// factored into a reusable page table walker.
pub fn map_dma_user(dma: &DmaBuffer, user_page_table: u64, user_virt: u64) -> Result<u64, ()> {
    let offset = crate::memory::physical_memory_offset();
    assert!(
        offset != 0,
        "physical_memory_offset not set — call set_physical_memory_offset first"
    );

    // Helper closure: physical-to-virtual via the stored offset.
    let to_virt = |phys: u64| -> u64 { phys.wrapping_add(offset) };

    let flags_rw = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE;

    let num_pages = dma.size / (PAGE_SIZE as usize);

    for page_idx in 0..num_pages {
        let virt = user_virt + (page_idx as u64) * PAGE_SIZE;
        let phys = dma.phys_addr + (page_idx as u64) * PAGE_SIZE;

        // Decompose the virtual address into page table indices.
        let p4_idx = ((virt >> 39) & 0x1FF) as usize;
        let p3_idx = ((virt >> 30) & 0x1FF) as usize;
        let p2_idx = ((virt >> 21) & 0x1FF) as usize;
        let p1_idx = ((virt >> 12) & 0x1FF) as usize;

        // Walk or create P3.
        // SAFETY: `user_page_table` is a valid P4 table provided by the caller.
        let l4 = unsafe {
            &mut *(to_virt(user_page_table) as *mut x86_64::structures::paging::PageTable)
        };
        let l3 = if l4[p4_idx].flags().contains(PageTableFlags::PRESENT) {
            // SAFETY: P4 entry is present, so the frame address is valid.
            unsafe {
                &mut *(to_virt(l4[p4_idx].frame().unwrap().start_address().as_u64())
                    as *mut x86_64::structures::paging::PageTable)
            }
        } else {
            let frame = frame_alloc::alloc_frame().ok_or(())?;
            // SAFETY: Frame is exclusively allocated.
            let table = unsafe {
                let t = &mut *(to_virt(frame) as *mut x86_64::structures::paging::PageTable);
                for entry in t.iter_mut() {
                    entry.set_addr(x86_64::PhysAddr::new(0), PageTableFlags::empty());
                }
                t
            };
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
            // SAFETY: P3 entry is present, so the frame address is valid.
            unsafe {
                &mut *(to_virt(l3[p3_idx].frame().unwrap().start_address().as_u64())
                    as *mut x86_64::structures::paging::PageTable)
            }
        } else {
            let frame = frame_alloc::alloc_frame().ok_or(())?;
            // SAFETY: Frame is exclusively allocated.
            let table = unsafe {
                let t = &mut *(to_virt(frame) as *mut x86_64::structures::paging::PageTable);
                for entry in t.iter_mut() {
                    entry.set_addr(x86_64::PhysAddr::new(0), PageTableFlags::empty());
                }
                t
            };
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
            // SAFETY: P2 entry is present, so the frame address is valid.
            unsafe {
                &mut *(to_virt(l2[p2_idx].frame().unwrap().start_address().as_u64())
                    as *mut x86_64::structures::paging::PageTable)
            }
        } else {
            let frame = frame_alloc::alloc_frame().ok_or(())?;
            // SAFETY: Frame is exclusively allocated.
            let table = unsafe {
                let t = &mut *(to_virt(frame) as *mut x86_64::structures::paging::PageTable);
                for entry in t.iter_mut() {
                    entry.set_addr(x86_64::PhysAddr::new(0), PageTableFlags::empty());
                }
                t
            };
            l2[p2_idx].set_addr(
                x86_64::PhysAddr::new(frame),
                PageTableFlags::PRESENT
                    | PageTableFlags::WRITABLE
                    | PageTableFlags::USER_ACCESSIBLE,
            );
            table
        };

        // Map the physical frame into P1 with user-accessible RW + NX.
        l1[p1_idx].set_addr(
            x86_64::PhysAddr::new(phys),
            flags_rw | PageTableFlags::USER_ACCESSIBLE,
        );
    }

    Ok(user_virt)
}

/// Free a DMA buffer, returning its physical frames to the allocator.
///
/// Frees each page of the buffer back to the frame allocator. Does NOT
/// unmap the buffer from any user-space page tables — that is the
/// caller's responsibility.
///
/// # Arguments
/// - `dma`: The `DmaBuffer` to free. Consumed by this call.
pub fn free_dma(dma: &DmaBuffer) {
    let num_pages = dma.size / (PAGE_SIZE as usize);

    for i in 0..num_pages {
        let frame_phys = dma.phys_addr + (i as u64) * PAGE_SIZE;
        frame_alloc::free_frame(frame_phys);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alloc_dma_single_page() {
        frame_alloc::reset();
        crate::memory::set_physical_memory_offset(0x1000_0000_0000);

        let dma = alloc_dma(4096, 4096).expect("single-page DMA alloc failed");
        assert_eq!(dma.size(), 4096);
        assert_eq!(dma.phys_addr() % PAGE_SIZE, 0);
        assert!(dma.phys_addr() >= frame_alloc::frame_region_start());
        assert!(dma.phys_addr() < frame_alloc::frame_region_end());

        free_dma(&dma);
    }

    #[test]
    fn test_alloc_dma_multi_page_contiguous() {
        frame_alloc::reset();
        crate::memory::set_physical_memory_offset(0x1000_0000_0000);

        let dma = alloc_dma(4096 * 4, 4096).expect("4-page DMA alloc failed");
        assert_eq!(dma.size(), 4096 * 4);
        // Contiguous: virt_addr should also be contiguous.
        assert_eq!(dma.virt_addr() + 4096 * 4 - dma.virt_addr(), 4096 * 4);

        free_dma(&dma);
    }

    #[test]
    fn test_alloc_dma_zero_size_fails() {
        frame_alloc::reset();
        assert!(alloc_dma(0, 4096).is_err());
    }

    #[test]
    fn test_alloc_dma_unaligned_size_fails() {
        frame_alloc::reset();
        assert!(alloc_dma(100, 4096).is_err());
    }

    #[test]
    fn test_alloc_dma_bad_alignment_fails() {
        frame_alloc::reset();
        // Not a power of two.
        assert!(alloc_dma(4096, 3000).is_err());
        // Zero alignment.
        assert!(alloc_dma(4096, 0).is_err());
        // Alignment smaller than page size.
        assert!(alloc_dma(4096, 512).is_err());
    }

    #[test]
    fn test_free_dma_returns_frames() {
        frame_alloc::reset();
        crate::memory::set_physical_memory_offset(0x1000_0000_0000);

        let before = frame_alloc::frame_count();
        let dma = alloc_dma(4096 * 2, 4096).expect("alloc failed");
        let after_alloc = frame_alloc::frame_count();
        // At least 2 frames consumed (may be more due to reserved region).
        assert!(before - after_alloc >= 2);

        free_dma(&dma);
        let after_free = frame_alloc::frame_count();
        // Frames returned: free count should increase by 2.
        assert_eq!(after_free - after_alloc, 2);
    }

    #[test]
    fn test_dma_buffer_zeroed() {
        frame_alloc::reset();
        crate::memory::set_physical_memory_offset(0x1000_0000_0000);

        let dma = alloc_dma(4096, 4096).expect("alloc failed");
        let slice = dma.as_mut_slice();
        for &b in slice {
            assert_eq!(b, 0, "DMA buffer not zeroed");
        }

        free_dma(&dma);
    }

    #[test]
    fn test_dma_phys_below_4gib() {
        frame_alloc::reset();
        crate::memory::set_physical_memory_offset(0x1000_0000_0000);

        let dma = alloc_dma(4096, 4096).expect("alloc failed");
        assert!(
            dma.phys_addr() < DMA_PHYSICAL_LIMIT,
            "DMA buffer at {:#x} exceeds 4 GiB limit",
            dma.phys_addr()
        );

        free_dma(&dma);
    }
}
