//! Memory management subsystem.
//!
//! Currently provides only a kernel heap allocator. Future work:
//!   - Physical frame allocator (bitmap or buddy system)
//!   - Virtual memory manager (page table manipulation)
//!   - Copy-on-write, demand paging, memory-mapped files

use crate::println;

pub mod allocator;

/// Initialize memory management. Must be called after GDT/IDT (so fault
/// handlers are in place) and before any subsystem that allocates.
pub fn init() {
    println!("[...] Initializing memory management");
    allocator::init_heap();
    println!("[OK] Heap allocator initialized");
}
