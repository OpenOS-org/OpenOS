//! Memory management subsystem.
//!
//! Provides the kernel heap allocator and physical-to-virtual address
//! translation using the bootloader's `physical_memory_offset`.
//!
//! Future work:
//!   - Physical frame allocator (bitmap or buddy system)
//!   - Virtual memory manager (page table manipulation)
//!   - Copy-on-write, demand paging, memory-mapped files

use core::sync::atomic::{AtomicU64, Ordering};

use crate::println;

pub mod allocator;

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
pub fn physical_memory_offset() -> u64 {
    PHYSICAL_MEMORY_OFFSET.load(Ordering::Acquire)
}

/// Convert a physical address to a virtual address using the bootloader's
/// physical memory offset.
///
/// # Safety
/// The physical address must be mapped by the bootloader at
/// `physical + offset`. This is guaranteed for all physical memory the
/// bootloader reports in its memory map.
///
/// # Panics
/// Panics if the physical memory offset has not been set (i.e., called
/// before `set_physical_memory_offset`).
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
    println!("[OK] Heap allocator initialized");
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
        set_physical_memory_offset(0);
        let virt = phys_to_virt(0x1234_5678);
        assert_eq!(virt, 0x1234_5678);
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
