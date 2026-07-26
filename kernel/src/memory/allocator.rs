//! Kernel heap allocator with dynamic growth.
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
//!
//! ## Dynamic growth
//!
//! The heap starts with a small static BSS region (64 KiB) for bootstrapping.
//! Once the frame allocator is available, the `GrowthAllocator` wrapper
//! intercepts allocation failures and grows the heap on demand by:
//!
//! 1. Allocating 4 KiB physical frames from the bitmap frame allocator
//! 2. Mapping them at contiguous virtual addresses right after the current
//!    heap end, using the kernel's page table
//! 3. Extending the `linked_list_allocator` heap with the new memory
//!
//! The heap is capped at [`MAX_HEAP_SIZE`] (16 MiB) to prevent runaway growth.

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use linked_list_allocator::Heap;
use x86_64::structures::paging::PageTableFlags;

use crate::memory::pagetable::{PageTable, PAGE_SIZE};

/// Initial heap size for bootstrapping (64 KiB).
///
/// This static BSS region is the only memory available before the frame
/// allocator is initialized. It must be large enough to satisfy early
/// allocations (GDT/IDT setup, initial data structures) until
/// `mark_growth_ready()` is called.
pub const INITIAL_HEAP_SIZE: usize = 64 * 1024;

/// Maximum total heap size (16 MiB).
///
/// Prevents runaway allocation from consuming all physical memory.
/// If this limit is reached, further allocations will return null.
pub const MAX_HEAP_SIZE: usize = 16 * 1024 * 1024;

/// Number of physical frames to allocate per growth step (16 frames = 64 KiB).
///
/// Each frame is 4 KiB, so one growth step adds 64 KiB to the heap.
/// This balances between minimizing growth calls and wasting physical memory.
const GROW_BLOCK_FRAMES: usize = 16;

/// Size in bytes of each growth step (64 KiB).
const GROW_BLOCK_SIZE: usize = GROW_BLOCK_FRAMES * PAGE_SIZE as usize;

/// Page table flags for kernel heap pages: present, writable, no-execute.
const HEAP_PAGE_FLAGS: PageTableFlags = PageTableFlags::PRESENT
    .union(PageTableFlags::WRITABLE)
    .union(PageTableFlags::NO_EXECUTE);

/// Static BSS region for the initial heap.
///
/// The bootloader maps all kernel segments contiguously, so this memory is
/// guaranteed to be accessible. Growth beyond this region uses physical
/// frames mapped via the kernel's page table at contiguous virtual addresses.
static mut INITIAL_HEAP_REGION: [u8; INITIAL_HEAP_SIZE] = [0; INITIAL_HEAP_SIZE];

/// Whether the frame allocator is available for heap growth.
///
/// Initially `false`; set to `true` by `mark_growth_ready()` after both the
/// heap and frame allocator are initialized.
static GROWTH_READY: AtomicBool = AtomicBool::new(false);

/// Total bytes currently available in the heap (initial + all growth).
///
/// Starts at [`INITIAL_HEAP_SIZE`] and grows by [`GROW_BLOCK_SIZE`] each time
/// the allocator needs more memory. Capped at [`MAX_HEAP_SIZE`].
static HEAP_TOTAL: AtomicUsize = AtomicUsize::new(INITIAL_HEAP_SIZE);

/// The virtual address immediately past the last byte of the current heap.
///
/// Used to determine where the next growth block should be mapped. For the
/// initial BSS region, this is set during `init_heap()`. After each growth
/// step, it advances by [`GROW_BLOCK_SIZE`].
static HEAP_END_VIRT: AtomicU64 = AtomicU64::new(0);

/// Guard against double initialization of the heap.
static HEAP_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// The global allocator.
///
/// In non-test mode, wraps a `GrowthAllocator` that dynamically grows the
/// heap when the inner `linked_list_allocator` runs out of memory.
///
/// In test mode, uses the system allocator since `init_heap()` is never called.
#[cfg(not(test))]
#[global_allocator]
static ALLOCATOR: GrowthAllocator = GrowthAllocator::new();

#[cfg(test)]
#[global_allocator]
static ALLOCATOR: std::alloc::System = std::alloc::System;

/// A wrapper around `Heap` that grows on demand when allocations fail.
///
/// Implements `GlobalAlloc` by first attempting the allocation on the inner
/// heap. If that fails and growth is possible, it allocates additional
/// physical frames, maps them at contiguous virtual addresses after the
/// current heap, extends the heap, and retries.
///
/// # Thread safety
///
/// The inner `spin::Mutex<Heap>` ensures that only one CPU can allocate or
/// grow at a time. Growth calls `frame_alloc::alloc_frame()` and the page
/// table's `map_page()`, each of which has its own independent lock, so
/// there is no deadlock risk.
struct GrowthAllocator {
    heap: spin::Mutex<Heap>,
}

impl GrowthAllocator {
    /// Create a new `GrowthAllocator` with an empty inner heap.
    const fn new() -> Self {
        Self {
            heap: spin::Mutex::new(Heap::empty()),
        }
    }

    /// Initialize the heap with the static BSS region.
    ///
    /// # Safety
    ///
    /// Must be called exactly once, before any allocations.
    unsafe fn init(&self) {
        // SAFETY: INITIAL_HEAP_REGION is a static BSS array, always mapped.
        // Single-init is guaranteed by the HEAP_INITIALIZED guard in init_heap().
        unsafe {
            self.heap.lock().init(
                (&raw mut INITIAL_HEAP_REGION).cast::<u8>(),
                INITIAL_HEAP_SIZE,
            );
        }
    }
}

/// Safety: The `GrowthAllocator` wraps a `spin::Mutex<Heap>` which provides
/// mutual exclusion. All heap operations are performed while holding the lock.
unsafe impl GlobalAlloc for GrowthAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut heap = self.heap.lock();

        // Fast path: try the existing heap.
        if let Ok(ptr) = heap.allocate_first_fit(layout) {
            return ptr.as_ptr();
        }

        // Slow path: attempt to grow the heap and retry.
        if GROWTH_READY.load(Ordering::Acquire) {
            // Grow enough to satisfy this allocation plus some headroom.
            let needed = layout.size().max(GROW_BLOCK_SIZE);
            if try_grow_locked(&mut heap, needed) {
                // Retry after growth.
                if let Ok(ptr) = heap.allocate_first_fit(layout) {
                    return ptr.as_ptr();
                }
            }
        }

        // Out of memory (or growth not available yet).
        core::ptr::null_mut()
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: ptr was returned by a previous alloc() call with the same
        // layout, as required by the GlobalAlloc contract.
        unsafe {
            self.heap
                .lock()
                .deallocate(core::ptr::NonNull::new_unchecked(ptr), layout);
        }
    }
}

/// Attempt to grow the heap by mapping additional physical frames.
///
/// Allocates physical frames from the frame allocator, maps them at
/// contiguous virtual addresses immediately after the current heap end
/// using the kernel's page table, and extends the heap with the new memory.
///
/// Returns `true` if the heap was successfully grown, `false` if no physical
/// frames are available or the maximum heap size would be exceeded.
///
/// # Arguments
///
/// - `heap`: A locked reference to the inner `Heap` (caller holds the lock).
/// - `min_bytes`: Minimum number of bytes to grow by (rounded up to
///   [`GROW_BLOCK_SIZE`]).
fn try_grow_locked(heap: &mut Heap, min_bytes: usize) -> bool {
    let current_total = HEAP_TOTAL.load(Ordering::Acquire);

    // Check if we've hit the maximum.
    if current_total >= MAX_HEAP_SIZE {
        return false;
    }

    // Determine how many frames to allocate. At least GROW_BLOCK_FRAMES,
    // but more if the allocation needs it.
    let bytes_needed = min_bytes.next_multiple_of(GROW_BLOCK_SIZE);
    let pages_needed = bytes_needed / (PAGE_SIZE as usize);

    // Don't exceed the maximum.
    let max_pages = (MAX_HEAP_SIZE - current_total) / (PAGE_SIZE as usize);
    let pages_to_alloc = pages_needed.min(max_pages);

    if pages_to_alloc == 0 {
        return false;
    }

    // Get the kernel's current page table from CR3.
    let (p4_frame, _) = x86_64::registers::control::Cr3::read();
    // SAFETY: The P4 frame from CR3 is the kernel's active page table.
    // We have exclusive access during this growth operation (the heap lock
    // prevents concurrent allocations, and we're in a single-threaded
    // context for page table modifications).
    let page_table = unsafe { PageTable::new(p4_frame.start_address().as_u64()) };

    let mut virt_addr = HEAP_END_VIRT.load(Ordering::Acquire);
    let mut frames_mapped: usize = 0;

    // Allocate and map frames one at a time.
    for _ in 0..pages_to_alloc {
        let frame_phys = match crate::frame_alloc::alloc_frame() {
            Some(f) => f,
            None => break, // Out of physical frames.
        };

        // Map the physical frame at the next contiguous virtual address
        // after the current heap end.
        if page_table
            .map_page(virt_addr, frame_phys, HEAP_PAGE_FLAGS)
            .is_err()
        {
            // Page table error (e.g., intermediate table allocation failed).
            // Free the frame we just allocated and stop growing.
            crate::frame_alloc::free_frame(frame_phys);
            break;
        }

        frames_mapped += 1;
        virt_addr += PAGE_SIZE;
    }

    if frames_mapped == 0 {
        return false;
    }

    let added_bytes = frames_mapped * (PAGE_SIZE as usize);

    // Extend the linked_list_allocator heap with the newly mapped memory.
    // SAFETY: The virtual memory from the previous heap end for `added_bytes`
    // is now backed by allocated physical frames that are mapped in the
    // kernel's page table. The addresses are contiguous in virtual space
    // (we mapped them sequentially). The memory is exclusively owned by the
    // heap and has the required lifetime.
    unsafe {
        heap.extend(added_bytes);
    }

    let new_total = current_total + added_bytes;
    HEAP_TOTAL.store(new_total, Ordering::Release);
    HEAP_END_VIRT.store(virt_addr, Ordering::Release);

    crate::serial_println!(
        "[heap] grew by {} KiB (total {} KiB / {} KiB max)",
        added_bytes / 1024,
        new_total / 1024,
        MAX_HEAP_SIZE / 1024,
    );

    true
}

/// Initialize the heap allocator using the static BSS region.
///
/// Sets up the initial 64 KiB heap in BSS. This is the only memory available
/// until `mark_growth_ready()` enables dynamic growth.
///
/// # Panics
///
/// Panics if called more than once (double-init corrupts the free list).
#[cfg(not(test))]
pub fn init_heap() {
    assert!(
        !HEAP_INITIALIZED.swap(true, Ordering::AcqRel),
        "init_heap called twice — heap already initialized"
    );

    // SAFETY: INITIAL_HEAP_REGION is a static BSS array, always mapped.
    // Single-init is guaranteed by the HEAP_INITIALIZED guard.
    unsafe {
        ALLOCATOR.init();
    }

    // Record the virtual end of the initial heap region.
    // SAFETY: INITIAL_HEAP_REGION is a valid static array.
    let region_start = (&raw mut INITIAL_HEAP_REGION).cast::<u8>() as u64;
    HEAP_END_VIRT.store(region_start + INITIAL_HEAP_SIZE as u64, Ordering::Release);
}

/// In test mode, `init_heap` is a no-op since we use the system allocator.
#[cfg(test)]
pub fn init_heap() {
    // No-op in test mode: the system allocator handles all allocations.
}

/// Mark the heap as ready for dynamic growth.
///
/// Must be called after the frame allocator has been initialized. Before this
/// call, allocation failures will not trigger heap growth.
pub fn mark_growth_ready() {
    GROWTH_READY.store(true, Ordering::Release);
}

/// Get the total heap capacity in bytes (initial + all growth).
#[must_use]
pub fn heap_capacity() -> usize {
    HEAP_TOTAL.load(Ordering::Acquire)
}

/// Get the amount of used heap bytes.
///
/// Returns 0 in test mode or if the heap is not yet initialized.
#[must_use]
pub fn heap_used() -> usize {
    #[cfg(not(test))]
    {
        ALLOCATOR.heap.lock().used()
    }
    #[cfg(test)]
    {
        0
    }
}

/// Get the amount of free heap bytes.
///
/// Returns 0 in test mode or if the heap is not yet initialized.
#[must_use]
pub fn heap_free() -> usize {
    #[cfg(not(test))]
    {
        ALLOCATOR.heap.lock().free()
    }
    #[cfg(test)]
    {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The initial heap size constant must be exactly 64 KiB.
    #[test]
    fn test_initial_heap_size_is_64kib() {
        assert_eq!(INITIAL_HEAP_SIZE, 64 * 1024);
    }

    /// The initial heap size must be a power of two for page alignment.
    #[test]
    fn test_initial_heap_size_is_power_of_two() {
        assert!(INITIAL_HEAP_SIZE.is_power_of_two());
    }

    /// The initial heap size must be at least 16 KiB (minimum useful heap).
    #[test]
    fn test_initial_heap_size_minimum() {
        assert!(INITIAL_HEAP_SIZE >= 16 * 1024);
    }

    /// The maximum heap size must be exactly 16 MiB.
    #[test]
    fn test_max_heap_size() {
        assert_eq!(MAX_HEAP_SIZE, 16 * 1024 * 1024);
    }

    /// The maximum heap size must be a power of two.
    #[test]
    fn test_max_heap_size_is_power_of_two() {
        assert!(MAX_HEAP_SIZE.is_power_of_two());
    }

    /// The maximum heap size must be larger than the initial heap size.
    #[test]
    fn test_max_heap_exceeds_initial() {
        assert!(MAX_HEAP_SIZE > INITIAL_HEAP_SIZE);
    }

    /// The grow block size must be a multiple of the page size (4 KiB).
    #[test]
    fn test_grow_block_size_page_aligned() {
        assert_eq!(GROW_BLOCK_SIZE % (PAGE_SIZE as usize), 0);
    }

    /// The grow block size must be at least one page.
    #[test]
    fn test_grow_block_size_minimum() {
        assert!(GROW_BLOCK_SIZE >= PAGE_SIZE as usize);
    }

    /// The grow block frames must match the grow block size.
    #[test]
    fn test_grow_block_frames_consistent() {
        assert_eq!(
            GROW_BLOCK_FRAMES * (PAGE_SIZE as usize),
            GROW_BLOCK_SIZE
        );
    }

    /// The grow block size must evenly divide the max heap size minus the
    /// initial heap size, so that growth steps align cleanly.
    #[test]
    fn test_grow_block_divides_growth_range() {
        let growth_range = MAX_HEAP_SIZE - INITIAL_HEAP_SIZE;
        assert_eq!(growth_range % GROW_BLOCK_SIZE, 0);
    }

    /// Heap page flags must include PRESENT.
    #[test]
    fn test_heap_page_flags_present() {
        assert!(HEAP_PAGE_FLAGS.contains(PageTableFlags::PRESENT));
    }

    /// Heap page flags must include WRITABLE.
    #[test]
    fn test_heap_page_flags_writable() {
        assert!(HEAP_PAGE_FLAGS.contains(PageTableFlags::WRITABLE));
    }

    /// Heap page flags must include NO_EXECUTE.
    #[test]
    fn test_heap_page_flags_no_execute() {
        assert!(HEAP_PAGE_FLAGS.contains(PageTableFlags::NO_EXECUTE));
    }

    /// `GROWTH_READY` starts as `false` (growth not available until bootstrapped).
    #[test]
    fn test_growth_ready_starts_false() {
        assert!(!GROWTH_READY.load(Ordering::Relaxed));
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

    /// `mark_growth_ready` must not panic (test mode is a no-op).
    #[test]
    fn test_mark_growth_ready_does_not_panic() {
        mark_growth_ready();
    }

    /// `heap_capacity` returns a reasonable value in test mode.
    #[test]
    fn test_heap_capacity_default() {
        let cap = heap_capacity();
        assert_eq!(cap, INITIAL_HEAP_SIZE);
    }

    /// `heap_used` returns 0 in test mode.
    #[test]
    fn test_heap_used_default() {
        assert_eq!(heap_used(), 0);
    }

    /// `heap_free` returns 0 in test mode.
    #[test]
    fn test_heap_free_default() {
        assert_eq!(heap_free(), 0);
    }

    /// `HEAP_END_VIRT` starts as 0 (set during init_heap).
    #[test]
    fn test_heap_end_virt_starts_zero() {
        assert_eq!(HEAP_END_VIRT.load(Ordering::Relaxed), 0);
    }
}
