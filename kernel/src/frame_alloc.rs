//! Simple bump frame allocator for user-space pages.
//!
//! Allocates 4 KiB physical frames from a reserved region. No free, no
//! coalesce — sufficient for a single-process demo. In a production kernel,
//! this would be replaced by a bitmap or buddy allocator that uses the
//! bootloader's memory map.
//!
//! ## Design
//!
//! The allocator hands out frames from a contiguous physical region:
//! - Start: `FRAME_REGION_START` (32 MiB — above the kernel)
//! - End: `FRAME_REGION_END` (64 MiB — 8K frames = 32 MiB)
//!
//! Each allocation bumps a cursor forward by 4096 bytes. When the cursor
//! reaches the end, `alloc_frame()` returns `None`.

use core::sync::atomic::{AtomicU64, Ordering};

/// Start of the physical frame region (32 MiB). Above the kernel image
/// (which loads at ~1 MiB) and below typical QEMU RAM limits.
const FRAME_REGION_START: u64 = 0x0200_0000;

/// End of the physical frame region (64 MiB). Provides 8K frames = 32 MiB
/// of user-space memory. Exceeding this returns `None`.
const FRAME_REGION_END: u64 = 0x0400_0000;

/// Cursor: next frame to allocate. Atomic for future multi-core safety.
static NEXT_FRAME: AtomicU64 = AtomicU64::new(FRAME_REGION_START);

/// Allocate a single 4 KiB physical frame.
///
/// Returns `Some(physical_address)` on success, `None` if the region is
/// exhausted. The returned frame is guaranteed to be:
/// - Within `[FRAME_REGION_START, FRAME_REGION_END)`
/// - Not previously allocated (bump semantics)
/// - Accessible via `phys_to_virt()` (the bootloader maps all physical memory)
pub fn alloc_frame() -> Option<u64> {
    let addr = NEXT_FRAME.fetch_add(0x1000, Ordering::Relaxed);
    if addr + 0x1000 > FRAME_REGION_END {
        None
    } else {
        Some(addr)
    }
}

/// Reset the allocator. Only for testing — in production, allocated frames
/// are never reclaimed.
pub fn reset() {
    NEXT_FRAME.store(FRAME_REGION_START, Ordering::Relaxed);
}
