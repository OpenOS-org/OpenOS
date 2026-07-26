//! Bitmap frame allocator for user-space pages.
//!
//! Allocates and frees 4 KiB physical frames from a reserved region using
//! a bitmap. Each bit represents one frame: 0 = free, 1 = allocated.
//!
//! ## Design
//!
//! - Region: 32 MiB to 64 MiB (8192 frames = 32 MiB of user memory)
//! - Bitmap: 1024 bytes (8192 bits, one per frame)
//! - `alloc_frame()`: scans bitmap for a free bit, sets it, returns frame address
//! - `free_frame(addr)`: clears the bit for the given frame
//!
//! The bitmap is stored in a static array (zero-initialized = all frames free).
//! In a production kernel, this would use the bootloader's memory map to
//! determine which frames are actually available.

use core::sync::atomic::{AtomicBool, Ordering};

/// Start of the physical frame region (32 MiB).
const FRAME_REGION_START: u64 = 0x0200_0000;

/// End of the physical frame region (64 MiB).
const FRAME_REGION_END: u64 = 0x0400_0000;

/// Total number of 4 KiB frames in the region.
const TOTAL_FRAMES: usize = ((FRAME_REGION_END - FRAME_REGION_START) / 0x1000) as usize;

/// Bitmap: 1 bit per frame. 0 = free, 1 = allocated.
/// Size: `TOTAL_FRAMES` / 8 bytes.
static BITMAP: spin::Mutex<[u8; TOTAL_FRAMES / 8]> = spin::Mutex::new([0u8; TOTAL_FRAMES / 8]);

/// Whether the allocator has been initialized.
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Initialize the frame allocator.
///
/// Marks frames 0-255 (first 1 MiB of the region) as reserved,
/// since they may overlap with kernel data. The rest are free.
pub fn init() {
    let mut bitmap = BITMAP.lock();
    // Mark the first 256 frames (1 MiB) as reserved.
    // These overlap with the region used by page table frames in task/user.rs.
    for i in 0..32 {
        bitmap[i] = 0xFF; // 8 frames per byte, 32 bytes = 256 frames
    }
    INITIALIZED.store(true, Ordering::Release);
}

/// Allocate a single 4 KiB physical frame.
///
/// Returns `Some(physical_address)` on success, `None` if no free frame.
/// The returned frame is guaranteed to be within `[FRAME_REGION_START, FRAME_REGION_END)`.
pub fn alloc_frame() -> Option<u64> {
    let mut bitmap = BITMAP.lock();
    for i in 0..bitmap.len() {
        if bitmap[i] != 0xFF {
            // Found a byte with at least one free bit.
            for bit in 0..8 {
                if bitmap[i] & (1 << bit) == 0 {
                    bitmap[i] |= 1 << bit;
                    let frame_idx = i * 8 + bit;
                    let addr = FRAME_REGION_START + (frame_idx as u64) * 0x1000;
                    return Some(addr);
                }
            }
        }
    }
    None
}

/// Free a previously allocated 4 KiB physical frame.
///
/// # Safety
/// `addr` must have been returned by `alloc_frame()` and not yet freed.
/// Freeing an unallocated or already-freed frame is undefined behavior
/// (corrupts the allocator state).
pub fn free_frame(addr: u64) {
    let frame_idx = ((addr - FRAME_REGION_START) / 0x1000) as usize;
    let byte_idx = frame_idx / 8;
    let bit_idx = frame_idx % 8;
    let mut bitmap = BITMAP.lock();
    bitmap[byte_idx] &= !(1 << bit_idx);
}

/// Reset the allocator. All frames become free.
/// Only for testing — in production, use `free_frame()` to release individual frames.
#[cfg(test)]
pub fn reset() {
    let mut bitmap = BITMAP.lock();
    for b in bitmap.iter_mut() {
        *b = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alloc_returns_valid_address() {
        reset();
        let frame = alloc_frame().unwrap();
        assert!(frame >= FRAME_REGION_START);
        assert!(frame < FRAME_REGION_END);
        assert_eq!(frame % 0x1000, 0);
    }

    #[test]
    fn test_alloc_sequential() {
        reset();
        let f1 = alloc_frame().unwrap();
        let f2 = alloc_frame().unwrap();
        let f3 = alloc_frame().unwrap();
        // Frames may not be sequential due to reserved region, but should be distinct.
        assert_ne!(f1, f2);
        assert_ne!(f2, f3);
        assert_ne!(f1, f3);
    }

    #[test]
    fn test_alloc_page_aligned() {
        reset();
        for _ in 0..100 {
            let frame = alloc_frame().unwrap();
            assert_eq!(frame % 0x1000, 0, "frame {:#x} not page-aligned", frame);
        }
    }

    #[test]
    fn test_alloc_no_overlap() {
        reset();
        let mut frames = Vec::new();
        for _ in 0..100 {
            frames.push(alloc_frame().unwrap());
        }
        frames.sort();
        for i in 1..frames.len() {
            assert!(frames[i] > frames[i - 1], "overlapping frames");
        }
    }

    #[test]
    fn test_free_and_realloc() {
        reset();
        let f1 = alloc_frame().unwrap();
        let _f2 = alloc_frame().unwrap();
        free_frame(f1);
        let f3 = alloc_frame().unwrap();
        assert_eq!(f1, f3); // should reuse the freed frame
    }

    #[test]
    fn test_alloc_exhaustion() {
        reset();
        let mut count = 0;
        while alloc_frame().is_some() {
            count += 1;
        }
        // Should get most frames (minus reserved ones).
        assert!(count > 7000, "only allocated {} frames", count);
        assert!(alloc_frame().is_none());
    }

    #[test]
    fn test_free_all_and_realloc() {
        reset();
        let mut frames = Vec::new();
        for _ in 0..100 {
            frames.push(alloc_frame().unwrap());
        }
        for f in &frames {
            free_frame(*f);
        }
        // All frames should be available again.
        for _ in 0..100 {
            assert!(alloc_frame().is_some());
        }
    }

    #[test]
    fn test_region_size() {
        assert_eq!(TOTAL_FRAMES, 8192);
    }
}
