//! User-space memory allocator.
//!
//! Provides a simple bump allocator for `#![no_std]` user-space programs.
//! The allocator uses a fixed-size static buffer as its heap.
//!
//! ## Design
//!
//! - **Bump allocation**: Advances a pointer on each allocation. Very fast
//!   (O(1) per allocation, no fragmentation).
//! - **No deallocation**: Memory is never freed. This is acceptable for
//!   short-lived processes or processes that allocate monotonically.
//! - **Fixed size**: The heap is a static array (default 64 KiB). For
//!   larger heaps, increase `HEAP_SIZE` or implement dynamic growth.
//!
//! ## Limitations
//!
//! - No deallocation (memory leaks are expected)
//! - Fixed heap size (cannot grow at runtime)

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

/// Heap size in bytes (64 KiB). Increase for programs that allocate more.
const HEAP_SIZE: usize = 64 * 1024;

/// The heap memory region. Placed in .bss (zero-initialized).
#[repr(align(4096))]
struct HeapMemory([u8; HEAP_SIZE]);

static HEAP: HeapMemory = HeapMemory([0; HEAP_SIZE]);

/// Bump allocator state.
pub struct BumpAllocator {
    /// Current allocation pointer (byte offset into the heap).
    offset: AtomicUsize,
}

impl BumpAllocator {
    /// Create a new bump allocator.
    const fn new() -> Self {
        Self {
            offset: AtomicUsize::new(0),
        }
    }
}

unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size();
        let align = layout.align();

        // Atomically bump the pointer with proper alignment.
        let mut old = self.offset.load(Ordering::Relaxed);
        loop {
            // Align the current offset.
            let aligned = (old + align - 1) & !(align - 1);
            let new = aligned + size;

            if new > HEAP_SIZE {
                // Out of memory.
                return core::ptr::null_mut();
            }

            match self
                .offset
                .compare_exchange_weak(old, new, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return HEAP.0.as_ptr().add(aligned).cast_mut(),
                Err(current) => old = current,
            }
        }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // Bump allocator does not free memory.
    }
}

/// Global allocator instance. Rust's `alloc` crate dispatches all
/// allocations through this.
#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator::new();

/// Returns the number of bytes remaining in the heap.
#[must_use]
pub fn remaining() -> usize {
    let used = ALLOCATOR.offset.load(Ordering::Relaxed);
    HEAP_SIZE.saturating_sub(used)
}

/// Returns the number of bytes allocated so far.
#[must_use]
pub fn allocated() -> usize {
    ALLOCATOR.offset.load(Ordering::Relaxed)
}
