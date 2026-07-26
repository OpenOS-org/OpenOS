//! Bitmap frame allocator for user-space pages.
//!
//! Allocates and frees 4 KiB physical frames from a reserved region using
//! a bitmap. Each bit represents one frame: 0 = free, 1 = allocated.
//!
//! ## Design
//!
//! - Region: configurable via `init_from_memory_map()`, defaults to 32-64 MiB
//! - Bitmap: 1024 bytes (8192 bits, one per frame)
//! - `alloc_frame()`: scans bitmap for a free bit, sets it, returns frame address
//! - `free_frame(addr)`: clears the bit for the given frame
//!
//! The bitmap is stored in a static array (zero-initialized = all frames free).
//! In a production kernel, this would use the bootloader's memory map to
//! determine which frames are actually available.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Page size (4 KiB).
const PAGE_SIZE: u64 = 0x1000;

/// Start of the physical frame region (default: 32 MiB).
static FRAME_REGION_START: AtomicU64 = AtomicU64::new(0x0200_0000);

/// End of the physical frame region (default: 64 MiB).
static FRAME_REGION_END: AtomicU64 = AtomicU64::new(0x0400_0000);

/// Maximum number of frames the bitmap can track.
/// This is sized for the default 32 MiB region (8192 frames).
const MAX_BITMAP_BYTES: usize = 1024;

/// Bitmap: 1 bit per frame. 0 = free, 1 = allocated.
/// Fixed-size array; only the first `bitmap_len` bytes are used.
static BITMAP: spin::Mutex<[u8; MAX_BITMAP_BYTES]> = spin::Mutex::new([0u8; MAX_BITMAP_BYTES]);

/// Actual number of bytes used in the bitmap (computed during init).
static BITMAP_LEN: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Total number of frames in the current region.
static TOTAL_FRAMES_COUNT: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

/// Whether the allocator has been initialized.
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Get the current frame region start address.
fn region_start() -> u64 {
    FRAME_REGION_START.load(Ordering::Acquire)
}

/// Get the current frame region end address.
fn region_end() -> u64 {
    FRAME_REGION_END.load(Ordering::Acquire)
}

/// Total number of 4 KiB frames in the current region.
fn total_frames() -> usize {
    ((region_end() - region_start()) / PAGE_SIZE) as usize
}

/// Initialize the frame allocator with default region (32-64 MiB).
///
/// Marks frames 0-255 (first 1 MiB of the region) as reserved,
/// since they may overlap with kernel data. The rest are free.
pub fn init() {
    let mut bitmap = BITMAP.lock();
    let tf = total_frames();
    let bm_len = tf.div_ceil(8);

    // Clear all bits.
    for b in bitmap.iter_mut() {
        *b = 0;
    }

    // Mark the first 256 frames (1 MiB) as reserved.
    // These overlap with the region used by page table frames in task/user.rs.
    for i in 0..32.min(bm_len) {
        bitmap[i] = 0xFF; // 8 frames per byte, 32 bytes = 256 frames
    }

    BITMAP_LEN.store(bm_len, Ordering::Release);
    TOTAL_FRAMES_COUNT.store(tf, Ordering::Release);
    INITIALIZED.store(true, Ordering::Release);
}

/// Initialize the frame allocator from the bootloader's memory map.
///
/// Scans the memory map for usable regions, selects the largest one,
/// and initializes the bitmap allocator to use that region.
///
/// # Arguments
/// - `memory_map`: An iterator of `(start_addr, end_addr, type)` tuples
///   representing memory regions from the bootloader's memory map.
///   Type 1 = usable RAM.
pub fn init_from_memory_map(memory_map: &[(u64, u64, u32)]) {
    // Find usable memory regions (type 1) above 1 MiB (avoid low memory).
    let min_addr: u64 = 0x0010_0000; // 1 MiB
    let max_addr: u64 = 0x4000_0000; // 1 GiB (limit to low memory for now)

    let mut best_start: u64 = 0x0200_0000;
    let mut best_end: u64 = 0x0400_0000;
    let mut best_size: u64 = best_end - best_start;

    for &(start, end, mem_type) in memory_map {
        // Only consider usable RAM (type 1).
        if mem_type != 1 {
            continue;
        }

        // Clamp to our address range.
        let region_start = start.max(min_addr);
        let region_end = end.min(max_addr);

        if region_start >= region_end {
            continue;
        }

        let region_size = region_end - region_start;
        if region_size > best_size {
            best_start = region_start;
            best_end = region_end;
            best_size = region_size;
        }
    }

    // Align to page boundaries.
    let aligned_start = (best_start + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let aligned_end = best_end & !(PAGE_SIZE - 1);

    if aligned_start >= aligned_end {
        // Fall back to default region.
        crate::serial_println!("[WARN] frame_alloc: no usable region in memory map, using default");
        init();
        return;
    }

    FRAME_REGION_START.store(aligned_start, Ordering::Release);
    FRAME_REGION_END.store(aligned_end, Ordering::Release);

    let mut bitmap = BITMAP.lock();
    let tf = ((aligned_end - aligned_start) / PAGE_SIZE) as usize;
    let bm_len = tf.div_ceil(8);

    // Ensure we don't exceed bitmap capacity.
    let effective_len = bm_len.min(MAX_BITMAP_BYTES);
    let effective_frames = effective_len * 8;

    // Clear all bits.
    for b in bitmap.iter_mut() {
        *b = 0;
    }

    // Mark the first 256 frames (1 MiB) as reserved if the region starts low.
    if aligned_start < 0x0020_0000 {
        for i in 0..32.min(effective_len) {
            bitmap[i] = 0xFF;
        }
    }

    BITMAP_LEN.store(effective_len, Ordering::Release);
    TOTAL_FRAMES_COUNT.store(effective_frames, Ordering::Release);
    FRAME_REGION_END.store(
        aligned_start + (effective_frames as u64) * PAGE_SIZE,
        Ordering::Release,
    );
    INITIALIZED.store(true, Ordering::Release);

    crate::serial_println!(
        "[OK] Frame allocator: {:#x}..{:#x} ({} frames, {} KiB bitmap)",
        aligned_start,
        aligned_start + (effective_frames as u64) * PAGE_SIZE,
        effective_frames,
        effective_len
    );
}

/// Initialize the frame allocator from the bootloader's memory regions.
///
/// Converts `MemoryRegion` structs from the bootloader API into the
/// `(start, end, type)` tuple format expected by `init_from_memory_map`.
///
/// # Arguments
/// - `regions`: A slice of `MemoryRegion` from `BootInfo::memory_regions`.
pub fn init_from_boot_info(regions: &[bootloader_api::info::MemoryRegion]) {
    use bootloader_api::info::MemoryRegionKind;

    let tuples: alloc::vec::Vec<(u64, u64, u32)> = regions
        .iter()
        .map(|r| {
            let kind: u32 = match r.kind {
                MemoryRegionKind::Usable => 1,
                _ => 0,
            };
            (r.start, r.end, kind)
        })
        .collect();

    init_from_memory_map(&tuples);
}

/// Allocate a single 4 KiB physical frame.
///
/// Returns `Some(physical_address)` on success, `None` if no free frame.
/// The returned frame is guaranteed to be within the configured region.
pub fn alloc_frame() -> Option<u64> {
    let mut bitmap = BITMAP.lock();
    let bm_len = BITMAP_LEN.load(Ordering::Acquire);
    let start = region_start();
    let end = region_end();

    for i in 0..bm_len {
        if bitmap[i] != 0xFF {
            // Found a byte with at least one free bit.
            for bit in 0..8 {
                if bitmap[i] & (1 << bit) == 0 {
                    bitmap[i] |= 1 << bit;
                    let frame_idx = i * 8 + bit;
                    let addr = start + (frame_idx as u64) * PAGE_SIZE;
                    // Bounds check: ensure allocated address is within region.
                    assert!(
                        addr < end,
                        "alloc_frame returned out-of-bounds address {addr:#x} (end={end:#x})"
                    );
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
    let start = region_start();
    let end = region_end();

    // Bounds check: address must be within the frame region.
    if addr < start || addr >= end {
        return;
    }

    let frame_idx = ((addr - start) / PAGE_SIZE) as usize;
    let byte_idx = frame_idx / 8;
    let bit_idx = frame_idx % 8;
    let mut bitmap = BITMAP.lock();

    // Double-free detection: if the bit is already clear, the frame was
    // never allocated or was already freed.
    assert!(
        bitmap[byte_idx] & (1 << bit_idx) != 0,
        "double-free detected at address {addr:#x}"
    );

    bitmap[byte_idx] &= !(1 << bit_idx);
}

/// Get the total number of frames in the allocator region.
pub fn frame_count() -> usize {
    TOTAL_FRAMES_COUNT.load(Ordering::Acquire)
}

/// Get the start address of the frame region.
pub fn frame_region_start() -> u64 {
    region_start()
}

/// Get the end address of the frame region.
pub fn frame_region_end() -> u64 {
    region_end()
}

/// Reset the allocator. All frames become free.
/// Only for testing -- in production, use `free_frame()` to release individual frames.
#[cfg(test)]
pub fn reset() {
    FRAME_REGION_START.store(0x0200_0000, Ordering::Release);
    FRAME_REGION_END.store(0x0400_0000, Ordering::Release);
    let mut bitmap = BITMAP.lock();
    for b in bitmap.iter_mut() {
        *b = 0;
    }
    let tf = total_frames();
    let bm_len = tf.div_ceil(8);
    BITMAP_LEN.store(bm_len, Ordering::Release);
    TOTAL_FRAMES_COUNT.store(tf, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alloc_returns_valid_address() {
        reset();
        let frame = alloc_frame().unwrap();
        assert!(frame >= region_start());
        assert!(frame < region_end());
        assert_eq!(frame % PAGE_SIZE, 0);
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
            assert_eq!(frame % PAGE_SIZE, 0, "frame {:#x} not page-aligned", frame);
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
        reset();
        assert_eq!(total_frames(), 8192);
    }

    #[test]
    fn test_init_from_memory_map() {
        // Simulate a bootloader memory map with a usable region.
        let memory_map = [
            (0x0000_0000u64, 0x000A_0000u64, 2u32), // reserved
            (0x0010_0000, 0x3FFF_F000, 1),          // usable: 1 MiB to ~1 GiB
        ];
        init_from_memory_map(&memory_map);

        let start = region_start();
        let end = region_end();
        assert!(start >= 0x0010_0000);
        assert!(end <= 0x3FFF_F000);
        assert_eq!(start % PAGE_SIZE, 0);
        assert_eq!(end % PAGE_SIZE, 0);
        assert!(end > start);

        // Should be able to allocate.
        let frame = alloc_frame().unwrap();
        assert!(frame >= start);
        assert!(frame < end);

        // Clean up.
        reset();
    }

    #[test]
    fn test_init_from_memory_map_no_usable() {
        // No usable regions -- should fall back to default.
        let memory_map = [(0x0000_0000u64, 0x0010_0000u64, 2u32)]; // reserved only
        init_from_memory_map(&memory_map);

        // Should still work with default region.
        let frame = alloc_frame().unwrap();
        assert!(frame >= 0x0200_0000);

        reset();
    }

    #[test]
    fn test_frame_count() {
        reset();
        assert_eq!(frame_count(), 8192);
    }

    #[test]
    fn test_frame_region_accessors() {
        reset();
        assert_eq!(frame_region_start(), 0x0200_0000);
        assert_eq!(frame_region_end(), 0x0400_0000);
    }
}
