//! LRU block cache for block devices.
//!
//! Caches up to 64 sectors (32 KiB) in memory. On a cache miss the least
//! recently used non-dirty entry is evicted and replaced. Dirty entries
//! are flushed to the underlying device before being overwritten, or in
//! bulk via [`flush_all`].
//!
//! ## Design
//!
//! - 64 fixed slots, each holding one 512-byte sector
//! - LRU eviction tracked by a monotonically increasing access counter
//! - Dirty flag per entry -- set by [`write_cached`], cleared on flush
//! - Device index + LBA uniquely identifies a cached sector

use alloc::boxed::Box;

use spin::Mutex;

use super::super::drivers::block;

/// Number of cache entries (slots).
const CACHE_SIZE: usize = 64;

/// Sector size in bytes.
const SECTOR_SIZE: usize = 512;

/// A single cached sector.
struct CacheEntry {
    /// Index into the global block device registry.
    device_idx: usize,
    /// Logical block address (sector number) on the device.
    lba: u64,
    /// Cached sector data.
    data: [u8; SECTOR_SIZE],
    /// `true` if `data` has been modified but not yet written back.
    dirty: bool,
    /// `true` if this slot holds valid cached data.
    valid: bool,
    /// Last access counter value (for LRU ordering).
    access_counter: u64,
}

impl CacheEntry {
    /// Create an empty (invalid) cache entry.
    const fn empty() -> Self {
        Self {
            device_idx: 0,
            lba: 0,
            data: [0u8; SECTOR_SIZE],
            dirty: false,
            valid: false,
            access_counter: 0,
        }
    }
}

/// Global block cache state.
///
/// Entries are heap-allocated to avoid large stack frames (`CACHE_SIZE` entries
/// at ~528 bytes each is ~33 KiB).
struct BlockCache {
    /// Heap-allocated array of cache entries.
    entries: Box<[CacheEntry; CACHE_SIZE]>,
    /// Monotonically increasing counter for LRU tracking.
    counter: u64,
}

impl BlockCache {
    /// Create a new empty cache with heap-allocated entries.
    fn new() -> Self {
        // Build a Vec of empty entries and convert to a boxed array.
        let mut entries = alloc::vec::Vec::with_capacity(CACHE_SIZE);
        for _ in 0..CACHE_SIZE {
            entries.push(CacheEntry::empty());
        }
        let boxed_slice = entries.into_boxed_slice();
        // SAFETY: We allocated exactly CACHE_SIZE entries.
        let entries: Box<[CacheEntry; CACHE_SIZE]> = boxed_slice
            .try_into()
            .unwrap_or_else(|_| unreachable!("CACHE_SIZE mismatch"));
        Self {
            entries,
            counter: 0,
        }
    }

    /// Bump the access counter and return the new value.
    fn next_counter(&mut self) -> u64 {
        self.counter = self.counter.wrapping_add(1);
        self.counter
    }

    /// Find the cache slot matching `(device_idx, lba)`.
    ///
    /// Returns `Some(index)` if found, `None` otherwise.
    fn find(&self, device_idx: usize, lba: u64) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| e.valid && e.device_idx == device_idx && e.lba == lba)
    }

    /// Find the least recently used non-dirty entry for eviction.
    ///
    /// Returns `Some(index)` of the LRU entry, or `None` if all entries
    /// are dirty (cannot evict).
    fn find_lru_victim(&self) -> Option<usize> {
        let mut best_idx: Option<usize> = None;
        let mut best_counter = u64::MAX;

        for (i, entry) in self.entries.iter().enumerate() {
            // Prefer invalid (empty) entries first -- no flush needed.
            if !entry.valid {
                return Some(i);
            }
            // Among valid entries, pick the one with the lowest counter.
            if !entry.dirty && entry.access_counter < best_counter {
                best_counter = entry.access_counter;
                best_idx = Some(i);
            }
        }

        best_idx
    }
}

// Global block cache, protected by a spin lock.
//
// Uses `lazy_static` because `BlockCache` cannot be constructed in a
// `const` context (heap allocation).
lazy_static::lazy_static! {
    static ref CACHE: Mutex<BlockCache> = Mutex::new(BlockCache::new());
}

/// Read a sector through the cache.
///
/// If the sector is cached, returns a copy of the data. On a cache miss,
/// reads from the device, caches the result, and returns it. Returns
/// `None` if the device read fails and no stale cache entry exists.
pub fn read_cached(device_idx: usize, lba: u64) -> Option<[u8; SECTOR_SIZE]> {
    let mut cache = CACHE.lock();

    // Cache hit.
    if let Some(idx) = cache.find(device_idx, lba) {
        let counter = cache.next_counter();
        cache.entries[idx].access_counter = counter;
        return Some(cache.entries[idx].data);
    }

    // Cache miss -- read from device while holding the lock (single-threaded
    // kernel, and the VirtIO driver has its own internal lock).
    let mut buf = [0u8; SECTOR_SIZE];
    let dev = block::get_device(device_idx)?;
    if dev.read_sector(lba, &mut buf).is_err() {
        return None;
    }

    // Insert into cache.
    let victim = cache.find_lru_victim()?;
    let counter = cache.next_counter();

    // If the victim is dirty, flush it first.
    let entry = &mut cache.entries[victim];
    if entry.valid && entry.dirty {
        if let Some(dev) = block::get_device(entry.device_idx) {
            let _ = dev.write_sector(entry.lba, &entry.data);
        }
    }

    let entry = &mut cache.entries[victim];
    entry.device_idx = device_idx;
    entry.lba = lba;
    entry.data = buf;
    entry.dirty = false;
    entry.valid = true;
    entry.access_counter = counter;

    Some(buf)
}

/// Write a sector through the cache.
///
/// Stores `data` in the cache and marks the entry dirty. Does **not**
/// immediately write to the device; use [`flush_all`] to persist.
///
/// Returns `Ok(())` on success, `Err(())` if no cache slot could be
/// allocated (all entries dirty).
pub fn write_cached(device_idx: usize, lba: u64, data: &[u8; SECTOR_SIZE]) -> Result<(), ()> {
    let mut cache = CACHE.lock();

    if let Some(idx) = cache.find(device_idx, lba) {
        // Update existing entry.
        let counter = cache.next_counter();
        cache.entries[idx].data = *data;
        cache.entries[idx].dirty = true;
        cache.entries[idx].access_counter = counter;
        return Ok(());
    }

    // Allocate a new slot.
    let victim = cache.find_lru_victim().ok_or(())?;
    let counter = cache.next_counter();

    // If the victim is dirty, flush it first.
    let entry = &mut cache.entries[victim];
    if entry.valid && entry.dirty {
        if let Some(dev) = block::get_device(entry.device_idx) {
            let _ = dev.write_sector(entry.lba, &entry.data);
        }
    }

    let entry = &mut cache.entries[victim];
    entry.device_idx = device_idx;
    entry.lba = lba;
    entry.data = *data;
    entry.dirty = true;
    entry.valid = true;
    entry.access_counter = counter;

    Ok(())
}

/// Flush all dirty cache entries to their respective block devices.
///
/// Clears the dirty flag on each successfully written entry. Returns the
/// number of entries that failed to flush.
pub fn flush_all() -> usize {
    let mut cache = CACHE.lock();
    let mut failures: usize = 0;

    for entry in &mut *cache.entries {
        if entry.valid && entry.dirty {
            if let Some(dev) = block::get_device(entry.device_idx) {
                if dev.write_sector(entry.lba, &entry.data).is_ok() {
                    entry.dirty = false;
                } else {
                    failures += 1;
                }
            } else {
                failures += 1;
            }
        }
    }

    failures
}
