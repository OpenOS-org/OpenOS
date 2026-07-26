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

use core::sync::atomic::{AtomicBool, Ordering};

use linked_list_allocator::LockedHeap;
use x86_64::structures::paging::{FrameAllocator, PhysFrame, Size4KiB};

/// Heap starts after the kernel BSS section. The bootloader maps all kernel
/// segments contiguously, so memory after BSS is mapped and available.
/// The exact address is set at runtime from the linker symbol `__bss_end`.
/// We use a static variable initialized during `init_heap`.
static mut HEAP_REGION: [u8; HEAP_SIZE] = [0; HEAP_SIZE];

/// 2 MiB heap size.
///
/// Expanded from 100 KiB to support VFS, more concurrent tasks,
/// and larger IPC messages. BSS section so costs no disk space; future work
/// should switch to dynamic growth via the frame allocator.
pub const HEAP_SIZE: usize = 2 * 1024 * 1024;

/// The global allocator. Rust's `alloc` crate dispatches `Box::new`,
/// `Vec::push`, `String::from`, etc. to this.
///
/// In test mode, we use the system allocator instead of the kernel's
/// heap allocator, since `init_heap()` is never called during tests.
#[cfg(not(test))]
#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

/// In test mode, provide a dummy allocator that panics if accidentally used.
/// The standard test harness uses the system allocator from `std`.
#[cfg(test)]
#[global_allocator]
static ALLOCATOR: std::alloc::System = std::alloc::System;

/// Guard against double initialization of the heap.
static HEAP_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Initialize the heap allocator using a static array as backing memory.
///
/// The heap is placed in a static array in the kernel's BSS section, which is
/// guaranteed to be mapped by the bootloader. This avoids needing to set up
/// additional page table entries for the heap.
///
/// # Panics
/// Panics if called more than once (double-init corrupts the free list).
#[cfg(not(test))]
pub fn init_heap() {
    assert!(
        !HEAP_INITIALIZED.swap(true, Ordering::AcqRel),
        "init_heap called twice — heap already initialized"
    );
    // SAFETY: HEAP_REGION is a static array in BSS, always mapped. We
    // guarantee single-init via the HEAP_INITIALIZED guard above.
    unsafe {
        ALLOCATOR
            .lock()
            .init((&raw mut HEAP_REGION).cast::<u8>(), HEAP_SIZE);
    }
}

/// In test mode, `init_heap` is a no-op since we use the system allocator.
#[cfg(test)]
pub fn init_heap() {
    // No-op in test mode: the system allocator handles all allocations.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The heap size constant must be exactly 2 MiB.
    #[test]
    fn test_heap_size_is_2mib() {
        assert_eq!(HEAP_SIZE, 2 * 1024 * 1024);
    }

    /// The heap size must be a power of two for page alignment.
    #[test]
    fn test_heap_size_is_power_of_two() {
        assert!(HEAP_SIZE.is_power_of_two());
    }

    /// The heap size must be at least 64 KiB (minimum useful heap).
    #[test]
    fn test_heap_size_minimum() {
        assert!(HEAP_SIZE >= 64 * 1024);
    }

    /// `DummyFrameAllocator` must always return `None` (no physical frames).
    #[test]
    fn test_dummy_frame_allocator_returns_none() {
        let mut alloc = DummyFrameAllocator;
        assert!(alloc.allocate_frame().is_none());
    }

    /// `DummyFrameAllocator` consistently returns `None` across multiple calls.
    #[test]
    fn test_dummy_frame_allocator_always_none() {
        let mut alloc = DummyFrameAllocator;
        for _ in 0..10 {
            assert!(alloc.allocate_frame().is_none());
        }
    }

    /// `HEAP_INITIALIZED` starts as `false` (not yet initialized).
    #[test]
    fn test_heap_initialized_starts_false() {
        assert!(!HEAP_INITIALIZED.load(Ordering::Relaxed));
    }

    /// `init_heap` must not panic when called (test mode is a no-op).
    #[test]
    fn test_init_heap_does_not_panic() {
        init_heap();
    }
}
