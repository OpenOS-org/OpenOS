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
