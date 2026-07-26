//! Kernel heap allocator.
//!
//! The kernel uses `linked_list_allocator`, a simple first-fit allocator that
//! maintains a linked list of free blocks. It's suitable for early kernel
//! development because:
//!   - No external dependencies (pure Rust, `no_std` compatible)
//!   - Thread-safe when wrapped in a `Mutex`
//!   - Reasonable performance for small, infrequent allocations
//!
//! A production kernel would use a slab allocator (for fixed-size objects
//! like tasks and IPC messages) or a buddy allocator (for page-granularity
//! physical memory).

use linked_list_allocator::LockedHeap;
use x86_64::structures::paging::{FrameAllocator, PhysFrame, Size4KiB};

/// Heap starts after the kernel BSS section. The bootloader maps all kernel
/// segments contiguously, so memory after BSS is mapped and available.
/// The exact address is set at runtime from the linker symbol `__bss_end`.
/// We use a static variable initialized during `init_heap`.
pub static mut HEAP_REGION: [u8; HEAP_SIZE] = [0; HEAP_SIZE];

/// 2 MiB heap — expanded from 100 KiB to support VFS, more concurrent tasks,
/// and larger IPC messages. BSS section so costs no disk space; future work
/// should switch to dynamic growth via the frame allocator.
pub const HEAP_SIZE: usize = 2 * 1024 * 1024;

/// The global allocator. Rust's `alloc` crate dispatches `Box::new`,
/// `Vec::push`, `String::from`, etc. to this.
#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

/// Initialize the heap allocator using a static array as backing memory.
///
/// The heap is placed in a static array in the kernel's BSS section, which is
/// guaranteed to be mapped by the bootloader. This avoids needing to set up
/// additional page table entries for the heap.
///
/// # Safety
/// - Must be called exactly once (double-init corrupts the free list).
pub fn init_heap() {
    // SAFETY: HEAP_REGION is a static array in BSS, always mapped. We
    // guarantee single-init by calling this exactly once from `memory::init()`.
    unsafe {
        ALLOCATOR
            .lock()
            .init((&raw mut HEAP_REGION).cast::<u8>(), HEAP_SIZE);
    }
}

/// Placeholder for a physical frame allocator.
///
/// A real implementation would maintain a bitmap or buddy-tree of free
/// physical pages, returned by the bootloader's memory map. For now, we
/// implement the trait with `None` so the type system accepts it, but any
/// attempt to allocate physical frames will fail cleanly.
pub struct DummyFrameAllocator;

// SAFETY: The `FrameAllocator` trait requires that `allocate_frame` returns
// a unique, non-aliased physical frame. Returning `None` is always safe —
// it signals "out of memory" rather than handing out a duplicate frame.
unsafe impl FrameAllocator<Size4KiB> for DummyFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        None
    }
}
