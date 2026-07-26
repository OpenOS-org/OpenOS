//! Virtual Memory Area (VMA) tracker.
//!
//! Each process maintains a list of VMAs describing its virtual address space.
//! VMAs are used to validate memory accesses, track heap growth, and support
//! future `mmap`/`munmap` syscalls.
//!
//! ## VMA types
//!
//! - `Code` — ELF text segment (RX)
//! - `Data` — ELF data/BSS segment (RW)
//! - `Stack` — User stack (RW)
//! - `Heap` — Program break region (RW)
//! - `Mmap` — Memory-mapped region (various permissions)

use alloc::vec::Vec;

use crate::memory::pagetable::PAGE_SIZE;

/// Permissions for a VMA region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmaFlags {
    /// Pages are readable.
    pub read: bool,
    /// Pages are writable.
    pub write: bool,
    /// Pages are executable.
    pub execute: bool,
}

impl VmaFlags {
    /// Read-only.
    pub const RDONLY: Self = Self {
        read: true,
        write: false,
        execute: false,
    };
    /// Read + Write (data/stack/heap).
    pub const RW: Self = Self {
        read: true,
        write: true,
        execute: false,
    };
    /// Read + Execute (code segments).
    pub const RX: Self = Self {
        read: true,
        write: false,
        execute: true,
    };
}

/// Type of virtual memory area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmaType {
    /// ELF code segment.
    Code,
    /// ELF data/BSS segment.
    Data,
    /// User stack.
    Stack,
    /// Program break (heap).
    Heap,
    /// Memory-mapped region.
    Mmap,
}

/// A contiguous virtual memory region with uniform permissions.
#[derive(Debug, Clone)]
pub struct VmaRegion {
    /// Start virtual address (page-aligned).
    pub start: u64,
    /// Size in bytes (page-aligned).
    pub size: u64,
    /// Permissions.
    pub flags: VmaFlags,
    /// Type of region.
    pub kind: VmaType,
}

impl VmaRegion {
    /// End address (exclusive).
    #[must_use]
    pub fn end(&self) -> u64 {
        self.start + self.size
    }

    /// Check if an address falls within this region.
    #[must_use]
    pub fn contains(&self, addr: u64) -> bool {
        addr >= self.start && addr < self.end()
    }

    /// Check if a range [addr, addr+len) is fully within this region.
    #[must_use]
    pub fn contains_range(&self, addr: u64, len: u64) -> bool {
        addr >= self.start && addr.saturating_add(len) <= self.end()
    }
}

/// Per-process virtual memory area list.
pub struct VmaList {
    regions: Vec<VmaRegion>,
}

impl VmaList {
    /// Create an empty VMA list.
    #[must_use]
    pub fn new() -> Self {
        Self {
            regions: Vec::new(),
        }
    }

    /// Add a new VMA region.
    ///
    /// Returns `Err` if the region overlaps with an existing one.
    pub fn add(&mut self, region: VmaRegion) -> Result<(), &'static str> {
        // Check for overlaps.
        for existing in &self.regions {
            if region.start < existing.end() && existing.start < region.end() {
                return Err("VMA region overlaps with existing region");
            }
        }
        self.regions.push(region);
        Ok(())
    }

    /// Remove the VMA containing `addr` (exact match on start address).
    ///
    /// Returns `true` if a region was removed.
    pub fn remove(&mut self, addr: u64) -> bool {
        if let Some(pos) = self.regions.iter().position(|r| r.start == addr) {
            self.regions.remove(pos);
            true
        } else {
            false
        }
    }

    /// Find the VMA region containing `addr`.
    #[must_use]
    pub fn find(&self, addr: u64) -> Option<&VmaRegion> {
        self.regions.iter().find(|r| r.contains(addr))
    }

    /// Check if `addr` is in a writable region.
    #[must_use]
    pub fn is_writable(&self, addr: u64) -> bool {
        self.find(addr).is_some_and(|r| r.flags.write)
    }

    /// Check if `addr` is in an executable region.
    #[must_use]
    pub fn is_executable(&self, addr: u64) -> bool {
        self.find(addr).is_some_and(|r| r.flags.execute)
    }

    /// Validate a memory access at `[addr, addr+len)`.
    ///
    /// Returns `true` if the entire range is within a VMA with the
    /// required permissions.
    #[must_use]
    pub fn validate_access(&self, addr: u64, len: u64, write: bool, execute: bool) -> bool {
        // Special case: zero-length access is always valid.
        if len == 0 {
            return true;
        }

        // The access might span multiple VMAs. Check each page.
        let mut cur = addr & !(PAGE_SIZE - 1); // Page-align down.
        let end = addr + len;

        while cur < end {
            if let Some(region) = self.find(cur) {
                if write && !region.flags.write {
                    return false;
                }
                if execute && !region.flags.execute {
                    return false;
                }
            } else {
                return false; // Not in any VMA.
            }
            cur += PAGE_SIZE;
        }
        true
    }

    /// Get the number of VMAs.
    #[must_use]
    pub fn len(&self) -> usize {
        self.regions.len()
    }

    /// Check if the VMA list is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// Get an iterator over all regions.
    pub fn iter(&self) -> core::slice::Iter<'_, VmaRegion> {
        self.regions.iter()
    }

    /// Get a mutable iterator over all regions.
    pub fn iter_mut(&mut self) -> core::slice::IterMut<'_, VmaRegion> {
        self.regions.iter_mut()
    }

    /// Find the heap VMA and extend it.
    ///
    /// Returns the old break address on success, or `None` if no heap VMA exists.
    pub fn extend_heap(&mut self, new_end: u64) -> Option<u64> {
        for region in &mut self.regions {
            if region.kind == VmaType::Heap {
                let old_end = region.end();
                let aligned = (new_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
                if aligned > old_end {
                    region.size = aligned - region.start;
                } else if aligned < region.start {
                    // Can't shrink below start.
                    return None;
                } else {
                    region.size = aligned - region.start;
                }
                return Some(old_end);
            }
        }
        None
    }
}

impl Default for VmaList {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> IntoIterator for &'a VmaList {
    type IntoIter = core::slice::Iter<'a, VmaRegion>;
    type Item = &'a VmaRegion;

    fn into_iter(self) -> Self::IntoIter {
        self.regions.iter()
    }
}

impl<'a> IntoIterator for &'a mut VmaList {
    type IntoIter = core::slice::IterMut<'a, VmaRegion>;
    type Item = &'a mut VmaRegion;

    fn into_iter(self) -> Self::IntoIter {
        self.regions.iter_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vma_region_contains() {
        let r = VmaRegion {
            start: 0x1000,
            size: 0x2000,
            flags: VmaFlags::RW,
            kind: VmaType::Data,
        };
        assert!(r.contains(0x1000));
        assert!(r.contains(0x2FFF));
        assert!(!r.contains(0x3000));
        assert!(!r.contains(0x0FFF));
    }

    #[test]
    fn test_vma_region_contains_range() {
        let r = VmaRegion {
            start: 0x1000,
            size: 0x2000,
            flags: VmaFlags::RW,
            kind: VmaType::Data,
        };
        assert!(r.contains_range(0x1000, 0x2000));
        assert!(r.contains_range(0x1500, 0x1000));
        assert!(!r.contains_range(0x1000, 0x3000)); // Extends beyond.
    }

    #[test]
    fn test_vma_list_add_and_find() {
        let mut list = VmaList::new();
        list.add(VmaRegion {
            start: 0x1000,
            size: 0x1000,
            flags: VmaFlags::RX,
            kind: VmaType::Code,
        })
        .unwrap();
        list.add(VmaRegion {
            start: 0x3000,
            size: 0x1000,
            flags: VmaFlags::RW,
            kind: VmaType::Data,
        })
        .unwrap();

        assert!(list.find(0x1000).is_some());
        assert!(list.find(0x1500).is_some());
        assert!(list.find(0x2000).is_none());
        assert!(list.find(0x3000).is_some());
    }

    #[test]
    fn test_vma_list_overlap_rejected() {
        let mut list = VmaList::new();
        list.add(VmaRegion {
            start: 0x1000,
            size: 0x2000,
            flags: VmaFlags::RW,
            kind: VmaType::Data,
        })
        .unwrap();
        let result = list.add(VmaRegion {
            start: 0x2000,
            size: 0x1000,
            flags: VmaFlags::RW,
            kind: VmaType::Heap,
        });
        assert!(result.is_err());
    }

    #[test]
    fn test_vma_list_remove() {
        let mut list = VmaList::new();
        list.add(VmaRegion {
            start: 0x1000,
            size: 0x1000,
            flags: VmaFlags::RW,
            kind: VmaType::Data,
        })
        .unwrap();
        assert!(list.remove(0x1000));
        assert!(list.find(0x1000).is_none());
        assert!(!list.remove(0x1000)); // Already removed.
    }

    #[test]
    fn test_vma_validate_access() {
        let mut list = VmaList::new();
        list.add(VmaRegion {
            start: 0x1000,
            size: 0x2000,
            flags: VmaFlags::RX,
            kind: VmaType::Code,
        })
        .unwrap();

        // Read is OK.
        assert!(list.validate_access(0x1000, 0x100, false, false));
        // Write is not OK (RX only).
        assert!(!list.validate_access(0x1000, 0x100, true, false));
        // Execute is OK.
        assert!(list.validate_access(0x1000, 0x100, false, true));
        // Unmapped address.
        assert!(!list.validate_access(0x5000, 0x100, false, false));
    }

    #[test]
    fn test_vma_is_writable() {
        let mut list = VmaList::new();
        list.add(VmaRegion {
            start: 0x1000,
            size: 0x1000,
            flags: VmaFlags::RW,
            kind: VmaType::Data,
        })
        .unwrap();
        assert!(list.is_writable(0x1000));
        assert!(!list.is_writable(0x5000));
    }

    #[test]
    fn test_vma_extend_heap() {
        let mut list = VmaList::new();
        list.add(VmaRegion {
            start: 0x5000,
            size: 0x1000,
            flags: VmaFlags::RW,
            kind: VmaType::Heap,
        })
        .unwrap();

        let old_end = list.extend_heap(0x7000);
        assert_eq!(old_end, Some(0x6000)); // Old end was 0x5000 + 0x1000.
        let region = list.find(0x5000).unwrap();
        assert_eq!(region.size, 0x2000); // New size: 0x7000 - 0x5000, page-aligned.
    }

    #[test]
    fn test_vma_flags_constants() {
        assert!(VmaFlags::RX.read);
        assert!(!VmaFlags::RX.write);
        assert!(VmaFlags::RX.execute);

        assert!(VmaFlags::RW.read);
        assert!(VmaFlags::RW.write);
        assert!(!VmaFlags::RW.execute);
    }

    #[test]
    fn test_vma_list_len() {
        let mut list = VmaList::new();
        assert_eq!(list.len(), 0);
        assert!(list.is_empty());

        list.add(VmaRegion {
            start: 0x1000,
            size: 0x1000,
            flags: VmaFlags::RW,
            kind: VmaType::Data,
        })
        .unwrap();
        assert_eq!(list.len(), 1);
        assert!(!list.is_empty());
    }

    #[test]
    fn test_vma_validate_zero_length() {
        let list = VmaList::new();
        // Zero-length access is always valid.
        assert!(list.validate_access(0x5000, 0, false, false));
    }
}
