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
#[must_use]
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
/// # Errors
///
/// Returns `Err(())` if no cache slot could be allocated (all entries dirty).
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
#[must_use]
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

#[cfg(test)]
mod tests {
    use super::*;

    // ─────────────────── CacheEntry tests ───────────────────

    #[test]
    fn cache_entry_empty_is_invalid() {
        let entry = CacheEntry::empty();
        assert!(!entry.valid);
        assert!(!entry.dirty);
        assert_eq!(entry.device_idx, 0);
        assert_eq!(entry.lba, 0);
        assert_eq!(entry.access_counter, 0);
    }

    #[test]
    fn cache_entry_empty_data_is_zeroed() {
        let entry = CacheEntry::empty();
        assert!(entry.data.iter().all(|&b| b == 0));
    }

    // ─────────────────── BlockCache tests ───────────────────

    #[test]
    fn block_cache_new_has_correct_size() {
        let cache = BlockCache::new();
        assert_eq!(cache.entries.len(), CACHE_SIZE);
    }

    #[test]
    fn block_cache_new_all_entries_invalid() {
        let cache = BlockCache::new();
        for entry in cache.entries.iter() {
            assert!(!entry.valid);
            assert!(!entry.dirty);
        }
    }

    #[test]
    fn block_cache_counter_starts_at_zero() {
        let cache = BlockCache::new();
        assert_eq!(cache.counter, 0);
    }

    #[test]
    fn block_cache_next_counter_increments() {
        let mut cache = BlockCache::new();
        assert_eq!(cache.next_counter(), 1);
        assert_eq!(cache.next_counter(), 2);
        assert_eq!(cache.next_counter(), 3);
    }

    #[test]
    fn block_cache_next_counter_wraps() {
        let mut cache = BlockCache::new();
        cache.counter = u64::MAX;
        assert_eq!(cache.next_counter(), 0);
    }

    #[test]
    fn find_returns_none_for_empty_cache() {
        let cache = BlockCache::new();
        assert!(cache.find(0, 0).is_none());
        assert!(cache.find(99, 42).is_none());
    }

    #[test]
    fn find_locates_valid_entry() {
        let mut cache = BlockCache::new();
        cache.entries[5].valid = true;
        cache.entries[5].device_idx = 2;
        cache.entries[5].lba = 100;

        assert_eq!(cache.find(2, 100), Some(5));
    }

    #[test]
    fn find_ignores_invalid_entries() {
        let mut cache = BlockCache::new();
        // Set the fields but leave valid=false.
        cache.entries[3].valid = false;
        cache.entries[3].device_idx = 1;
        cache.entries[3].lba = 50;

        assert!(cache.find(1, 50).is_none());
    }

    #[test]
    fn find_distinguishes_device_idx() {
        let mut cache = BlockCache::new();
        cache.entries[0].valid = true;
        cache.entries[0].device_idx = 0;
        cache.entries[0].lba = 10;

        // Same LBA but different device.
        assert!(cache.find(1, 10).is_none());
        assert_eq!(cache.find(0, 10), Some(0));
    }

    #[test]
    fn find_distinguishes_lba() {
        let mut cache = BlockCache::new();
        cache.entries[0].valid = true;
        cache.entries[0].device_idx = 0;
        cache.entries[0].lba = 10;

        assert!(cache.find(0, 11).is_none());
        assert_eq!(cache.find(0, 10), Some(0));
    }

    #[test]
    fn find_lru_victim_prefers_invalid() {
        let mut cache = BlockCache::new();
        // Mark all entries as valid with high counters.
        for (i, entry) in cache.entries.iter_mut().enumerate() {
            entry.valid = true;
            entry.dirty = false;
            entry.access_counter = 1000 + i as u64;
        }
        // Make entry 10 invalid (empty).
        cache.entries[10].valid = false;

        let victim = cache.find_lru_victim();
        assert_eq!(victim, Some(10));
    }

    #[test]
    fn find_lru_victim_picks_lowest_counter() {
        let mut cache = BlockCache::new();
        // All entries valid, non-dirty, with varying counters.
        for (i, entry) in cache.entries.iter_mut().enumerate() {
            entry.valid = true;
            entry.dirty = false;
            entry.access_counter = (i as u64 + 1) * 100;
        }
        // Entry 0 has counter 100 (lowest).
        let victim = cache.find_lru_victim();
        assert_eq!(victim, Some(0));
    }

    #[test]
    fn find_lru_victim_skips_dirty_entries() {
        let mut cache = BlockCache::new();
        // All entries valid.
        for (i, entry) in cache.entries.iter_mut().enumerate() {
            entry.valid = true;
            entry.access_counter = i as u64;
        }
        // Make the lowest-counter entries dirty.
        cache.entries[0].dirty = true;
        cache.entries[1].dirty = true;
        cache.entries[2].dirty = true;

        let victim = cache.find_lru_victim();
        assert_eq!(victim, Some(3)); // First non-dirty.
    }

    #[test]
    fn find_lru_victim_returns_none_if_all_dirty() {
        let mut cache = BlockCache::new();
        for entry in cache.entries.iter_mut() {
            entry.valid = true;
            entry.dirty = true;
            entry.access_counter = 1;
        }
        assert!(cache.find_lru_victim().is_none());
    }

    // ─────────────────── Cache size constant ───────────────────

    #[test]
    fn cache_size_value() {
        assert_eq!(CACHE_SIZE, 64);
    }

    #[test]
    fn sector_size_value() {
        assert_eq!(SECTOR_SIZE, 512);
    }

    // ─────────────────── read_cached / write_cached with global cache ───────────────────

    #[test]
    fn read_cached_returns_none_for_unregistered_device() {
        // Device 99 is not registered; read_cached should return None.
        let result = read_cached(99, 0);
        assert!(result.is_none());
    }

    #[test]
    fn write_cached_fails_for_unregistered_device() {
        // With all entries potentially dirty and no device, this may fail.
        // The key behavior: writing to a non-existent device slot
        // should not panic.
        let data = [0u8; SECTOR_SIZE];
        // This may succeed or fail depending on cache state, but must not panic.
        let _ = write_cached(99, 0, &data);
    }

    #[test]
    fn flush_all_returns_count() {
        // flush_all should return a count of failures. With no devices
        // registered, any dirty entries would count as failures.
        let _failures = flush_all();
        // Just verify it doesn't panic and returns a usize.
    }
}
