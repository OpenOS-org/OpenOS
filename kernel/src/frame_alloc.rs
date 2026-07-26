//! Bitmap frame allocator for user-space pages.
//!
//! Allocates and frees 4 KiB physical frames from a reserved region using
//! a bitmap. Each bit represents one frame: 0 = free, 1 = allocated.
//!
//! ## Bitmap layout
//!
//! The bitmap is a fixed-size `[u8; 1024]` array (8192 bits). Each bit
//! corresponds to one 4 KiB physical frame:
//!
//! ```text
//!   Byte 0:  [frame7 frame6 frame5 frame4 frame3 frame2 frame1 frame0]
//!   Byte 1:  [frame15 ... frame8]
//!   ...
//!   Byte N:  [frame(N*8+7) ... frame(N*8)]
//! ```
//!
//! Bit ordering: bit 0 of byte N = frame (N*8), bit 7 = frame (N*8+7).
//!
//! ## Memory region management
//!
//! The allocator manages a contiguous physical address range:
//! - `FRAME_REGION_START`: start address (default 32 MiB, `0x0200_0000`)
//! - `FRAME_REGION_END`: end address (default 64 MiB, `0x0400_0000`)
//! - Frame index = `(address - FRAME_REGION_START) / PAGE_SIZE`
//!
//! The region can be reconfigured via `init_from_memory_map()` to use the
//! largest usable RAM region reported by the bootloader. The first 256 frames
//! (1 MiB) are reserved to avoid overlapping with kernel data structures.
//!
//! ## Thread safety
//!
//! The bitmap is protected by a `spin::Mutex`. All public functions acquire
//! the lock for the duration of their operation.

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
///
/// Computed as `(region_end - region_start) / PAGE_SIZE`. This is the
/// theoretical maximum; the effective count may be smaller if the bitmap
/// capacity is exceeded.
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
    // These overlap with the region used by page table frames in task/user.rs
    // and may contain kernel data structures. 32 bytes * 8 bits = 256 frames.
    for i in 0..32.min(bm_len) {
        bitmap[i] = 0xFF;
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
    // Guard against double initialization.
    if INITIALIZED.load(Ordering::Acquire) {
        return;
    }

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
/// Scans the bitmap for the first free bit (0), sets it to 1, and returns
/// the corresponding physical address. Returns `None` if all frames are
/// allocated.
///
/// The scan is linear (first-fit) starting from byte 0. The first 256 frames
/// (1 MiB) are reserved during init, so early allocations start after that.
///
/// Returns `Some(physical_address)` on success, `None` if no free frame.
/// The returned frame is guaranteed to be page-aligned and within the
/// configured region.
///
/// # Panics
///
/// Panics if the computed address falls outside the configured region,
/// which would indicate internal bitmap corruption.
pub fn alloc_frame() -> Option<u64> {
    let mut bitmap = BITMAP.lock();
    let bm_len = BITMAP_LEN.load(Ordering::Acquire);
    let start = region_start();
    let end = region_end();

    // Linear scan: find the first byte that isn't all-ones (0xFF).
    for i in 0..bm_len {
        if bitmap[i] != 0xFF {
            // Found a byte with at least one free bit.
            // Scan individual bits (LSB first = lowest frame index first).
            for bit in 0..8 {
                if bitmap[i] & (1 << bit) == 0 {
                    // Mark the frame as allocated.
                    bitmap[i] |= 1 << bit;
                    let frame_idx = i * 8 + bit;
                    let addr = start + (frame_idx as u64) * PAGE_SIZE;
                    // Sanity check: allocated address must be within the region.
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
/// Clears the corresponding bit in the bitmap, marking the frame as available
/// for future allocation. The address is validated against the configured region.
///
/// # Safety contract
///
/// `addr` must satisfy:
/// - It was previously returned by `alloc_frame()`.
/// - It has not already been freed (no double-free).
///
/// Violating this contract triggers a panic (double-free detection) rather
/// than silent corruption. This is deliberate: double-frees indicate a
/// logic bug that must be caught immediately.
///
/// # Behavior on invalid addresses
///
/// If `addr` is outside the configured frame region, the call is silently
/// ignored (no-op). This is safe because such addresses were never allocated
/// by this allocator.
///
/// # Panics
///
/// Panics if the address is within the region but the corresponding bit
/// is already clear (double-free detection).
pub fn free_frame(addr: u64) {
    let start = region_start();
    let end = region_end();

    // Bounds check: address must be within the frame region.
    // Silently ignore addresses outside the region (they were never allocated).
    if addr < start || addr >= end {
        return;
    }

    // Convert address to bitmap indices.
    // frame_idx = linear index within the region (0-based).
    // byte_idx = which byte in the bitmap contains this frame's bit.
    // bit_idx = which bit within that byte (0 = LSB = lowest frame).
    let frame_idx = ((addr - start) / PAGE_SIZE) as usize;
    let byte_idx = frame_idx / 8;
    let bit_idx = frame_idx % 8;
    let mut bitmap = BITMAP.lock();

    // Double-free detection: the bit must be set (frame was allocated).
    // If it's already clear, this is a double-free or a bogus address.
    assert!(
        bitmap[byte_idx] & (1 << bit_idx) != 0,
        "double-free detected at address {addr:#x} (frame_idx={frame_idx}, byte={byte_idx}, bit={bit_idx})"
    );

    // Clear the bit to mark the frame as free.
    bitmap[byte_idx] &= !(1 << bit_idx);
}

/// Get the total number of frames in the allocator region.
#[must_use]
pub fn frame_count() -> usize {
    TOTAL_FRAMES_COUNT.load(Ordering::Acquire)
}

/// Get the start address of the frame region.
#[must_use]
pub fn frame_region_start() -> u64 {
    region_start()
}

/// Get the end address of the frame region.
#[must_use]
pub fn frame_region_end() -> u64 {
    region_end()
}

/// Reset the allocator. All frames become free.
/// Only for testing -- in production, use `free_frame()` to release individual frames.
#[cfg(test)]
pub fn reset() {
    INITIALIZED.store(false, Ordering::Release);
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
