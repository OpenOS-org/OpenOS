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

/// Heap starts at a high virtual address to avoid colliding with the kernel
/// image (at 0xFFFFFFFF80100000) and future user-space mappings (at 0x0).
/// The address is arbitrary — just needs to be in a mapped page.
pub const HEAP_START: usize = 0x_4444_4444_0000;

/// 100 KiB is enough for early development. Will need to grow dynamically
/// once we have a proper page allocator.
pub const HEAP_SIZE: usize = 100 * 1024;

/// The global allocator. Rust's `alloc` crate dispatches `Box::new`,
/// `Vec::push`, `String::from`, etc. to this.
#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

/// Initialize the heap allocator with the memory region at `[HEAP_START, HEAP_START+HEAP_SIZE)`.
///
/// # Safety
/// - Must be called exactly once (double-init corrupts the free list).
/// - Must be called after the page tables map `HEAP_START..HEAP_START+HEAP_SIZE`
///   to valid physical frames (the bootloader does this for the first 2 MiB,
///   which covers our heap address).
pub fn init_heap() {
    // SAFETY: `init` writes a free-list header at `HEAP_START`. The memory
    // region must be mapped and not used by anything else. We guarantee
    // single-init by calling this exactly once from `memory::init()`.
    unsafe {
        ALLOCATOR.lock().init(HEAP_START as *mut u8, HEAP_SIZE);
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
