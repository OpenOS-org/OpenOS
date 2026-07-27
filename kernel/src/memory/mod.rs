//! Memory management subsystem.
//!
//! Provides:
//! - Kernel heap allocator (`allocator`) — dynamic-growth `linked_list_allocator`
//! - Physical-to-virtual address translation via `physical_memory_offset`
//! - Bitmap frame allocator (`frame_alloc`) — 4 KiB physical frames
//! - Unified page table abstraction (`pagetable`) — map/unmap/translate
//! - Virtual Memory Area tracker (`vma`) — per-process address space regions
//! - DMA buffer allocation (`dma`) — physically contiguous, <4 GiB

use core::sync::atomic::{AtomicU64, Ordering};

use x86_64::structures::paging::PageTableFlags;

use crate::println;

pub mod allocator;
pub mod dma;
pub mod pagetable;
pub mod vma;

/// Maximum user-space virtual address. Pointers at or above this are in
/// kernel space and must not be dereferenced on behalf of user code.
pub const USER_SPACE_MAX: u64 = 0x0000_8000_0000_0000;

/// Physical memory offset from `BootInfo`. The bootloader maps all physical
/// memory at `virtual = physical + offset`. We store it here so any module
/// can convert physical → virtual without passing `BootInfo` through every call chain.
static PHYSICAL_MEMORY_OFFSET: AtomicU64 = AtomicU64::new(0);

/// Set the physical memory offset. Must be called once during early boot
/// from `kernel_main` before any page table walks.
pub fn set_physical_memory_offset(offset: u64) {
    PHYSICAL_MEMORY_OFFSET.store(offset, Ordering::Release);
}

/// Get the physical memory offset. Returns 0 if not yet set.
#[must_use]
pub fn physical_memory_offset() -> u64 {
    PHYSICAL_MEMORY_OFFSET.load(Ordering::Acquire)
}

/// Convert a physical address to a virtual address using the bootloader's
/// physical memory offset.
///
/// # Safety
///
/// The physical address must be mapped by the bootloader at
/// `physical + offset`. This is guaranteed for all physical memory the
/// bootloader reports in its memory map.
///
/// # Panics
///
/// Panics if the physical memory offset has not been set (i.e., called
/// before `set_physical_memory_offset`).
#[must_use]
pub fn phys_to_virt(phys: u64) -> u64 {
    let offset = physical_memory_offset();
    assert!(
        offset != 0,
        "physical_memory_offset not set — call set_physical_memory_offset first"
    );
    phys.wrapping_add(offset)
}

/// Initialize memory management. Must be called after GDT/IDT (so fault
/// handlers are in place) and before any subsystem that allocates.
pub fn init() {
    println!("[...] Initializing memory management");
    allocator::init_heap();
    crate::frame_alloc::init();
    allocator::mark_growth_ready();
    println!("[OK] Heap allocator initialized (64 KiB initial, 16 MiB max, dynamic growth)");
    println!("[OK] Frame allocator initialized");
}

/// Initialize memory management using the bootloader's memory map.
///
/// This variant uses the actual memory map from `BootInfo` to configure
/// the frame allocator, selecting the largest usable region instead of
/// the hardcoded 32-64 MiB default.
pub fn init_with_memory_map(memory_map: &[(u64, u64, u32)]) {
    println!("[...] Initializing memory management");
    allocator::init_heap();
    crate::frame_alloc::init_from_memory_map(memory_map);
    allocator::mark_growth_ready();
    println!("[OK] Heap allocator initialized (64 KiB initial, 16 MiB max, dynamic growth)");
    println!("[OK] Frame allocator initialized");
}

/// Create a new page table for a user process.
///
/// Allocates a fresh P4 page table and copies the kernel's higher-half
/// entries (indices 256..512) from the current page table. The lower
/// half (user space) is left empty for the process to populate.
///
/// Returns the physical address of the new P4 table, or `None` if
/// allocation fails.
#[must_use]
pub fn create_user_page_table() -> Option<u64> {
    use x86_64::registers::control::Cr3;
    use x86_64::structures::paging::PageTable;

    let p4_frame = crate::frame_alloc::alloc_frame()?;
    let p4_virt = phys_to_virt(p4_frame) as *mut PageTable;

    // SAFETY: `p4_frame` was just allocated, so it's exclusively owned.
    // We write to it via the physical memory mapping.
    unsafe {
        let new_p4 = &mut *p4_virt;

        // Zero the entire table first.
        for entry in new_p4.iter_mut() {
            entry.set_addr(x86_64::PhysAddr::new(0), PageTableFlags::empty());
        }

        // Copy ALL P4 entries (0..512) from the current kernel page table.
        // The physical memory mapping may reside at a P4 index below 256
        // (e.g., index 2 for physical_memory_offset 0x10000000000), and
        // phys_to_virt needs it to be accessible during ELF loading.
        let (current_p4_frame, _) = Cr3::read();
        let current_p4_virt =
            phys_to_virt(current_p4_frame.start_address().as_u64()) as *const PageTable;
        let current_p4 = &*current_p4_virt;

        for i in 0..512 {
            new_p4[i].set_addr(current_p4[i].addr(), current_p4[i].flags());
        }
    }

    Some(p4_frame)
}

/// Switch the current page table (load CR3).
///
/// # Safety
/// `p4_phys` must be the physical address of a valid P4 page table.
/// The table must have valid kernel mappings in the higher half.
pub unsafe fn switch_page_table(p4_phys: u64) {
    use x86_64::registers::control::Cr3;
    use x86_64::structures::paging::PhysFrame;
    use x86_64::PhysAddr;

    // SAFETY: The caller guarantees `p4_phys` is a valid P4 table.
    unsafe {
        let frame = PhysFrame::containing_address(PhysAddr::new(p4_phys));
        Cr3::write(frame, Cr3::read().1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_space_max() {
        // USER_SPACE_MAX should be 128 TiB (half of 256 TiB virtual address space).
        assert_eq!(USER_SPACE_MAX, 0x0000_8000_0000_0000);
    }

    #[test]
    fn test_set_and_get_physical_memory_offset() {
        // Reset to 0 first.
        PHYSICAL_MEMORY_OFFSET.store(0, Ordering::Release);
        assert_eq!(physical_memory_offset(), 0);

        set_physical_memory_offset(0x1000_0000_0000);
        assert_eq!(physical_memory_offset(), 0x1000_0000_0000);

        // Clean up.
        PHYSICAL_MEMORY_OFFSET.store(0, Ordering::Release);
    }

    #[test]
    fn test_phys_to_virt_basic() {
        set_physical_memory_offset(0x1000_0000_0000);
        let virt = phys_to_virt(0x1000_0000);
        assert_eq!(virt, 0x1000_0000_0000 + 0x1000_0000);
    }

    #[test]
    fn test_phys_to_virt_zero_phys() {
        set_physical_memory_offset(0xFFFF_8000_0000_0000);
        let virt = phys_to_virt(0);
        assert_eq!(virt, 0xFFFF_8000_0000_0000);
    }

    #[test]
    fn test_phys_to_virt_identity() {
        // Use a small offset to test address translation.
        set_physical_memory_offset(0x1000);
        let virt = phys_to_virt(0x1234_5678);
        assert_eq!(virt, 0x1234_5678 + 0x1000);
    }

    #[test]
    #[should_panic(expected = "physical_memory_offset not set")]
    fn test_phys_to_virt_panics_without_offset() {
        PHYSICAL_MEMORY_OFFSET.store(0, Ordering::Release);
        phys_to_virt(0x1000);
    }

    #[test]
    fn test_phys_to_virt_high_address() {
        set_physical_memory_offset(0x1000_0000_0000);
        // A high physical address (e.g., 128 GiB).
        let virt = phys_to_virt(0x20_0000_0000);
        assert_eq!(virt, 0x1000_0000_0000 + 0x20_0000_0000);
    }

    #[test]
    fn test_phys_to_virt_wrapping() {
        // With a very large offset, the addition wraps.
        set_physical_memory_offset(0xFFFF_FFFF_FFFF_0000);
        let virt = phys_to_virt(0x2000);
        assert_eq!(virt, 0xFFFF_FFFF_FFFF_0000u64.wrapping_add(0x2000));
    }
}
