//! Virtual Memory Area (VMA) tracker.
//!
//! Each process maintains a list of VMAs describing its virtual address space.
#![allow(
    missing_docs,
    clippy::unnecessary_cast,
    clippy::manual_div_ceil,
    clippy::empty_line_after_doc_comments,
    clippy::needless_for_each
)]
//! VMAs are used to validate memory accesses, track heap growth, and support
//! `mmap`/`munmap` syscalls.
//!
//! ## VMA types
//!
//! - `Code` — ELF text segment (RX)
//! - `Data` — ELF data/BSS segment (RW)
//! - `Stack` — User stack (RW)
//! - `Heap` — Program break region (RW)
//! - `Mmap` — Anonymous memory-mapped region
//! - `FileMmap` — File-backed memory-mapped region

use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use crate::memory::pagetable::PAGE_SIZE;

/// Page size as `u64` for alignment arithmetic.
const PAGE_SIZE_U64: u64 = PAGE_SIZE as u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Permission flags for a VMA region.
#[allow(missing_docs)]
pub struct VmaFlags {
    /// Read permission.
    pub read: bool,
    /// Write permission.
    pub write: bool,
    /// Execute permission.
    pub execute: bool,
}

impl VmaFlags {
    /// Read-only.
    pub const RDONLY: Self = Self {
        read: true,
        write: false,
        execute: false,
    };
    /// Read-Write.
    pub const RW: Self = Self {
        read: true,
        write: true,
        execute: false,
    };
    /// Read-Execute.
    pub const RX: Self = Self {
        read: true,
        write: false,
        execute: true,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Type of a VMA region.
pub enum VmaType {
    /// ELF code segment.
    Code,
    /// ELF data/BSS segment.
    Data,
    /// User stack.
    Stack,
    /// Program heap (brk).
    Heap,
    /// Anonymous mmap region.
    Mmap,
    /// File-backed mmap region.
    FileMmap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// mmap flags (MAP_* constants).
pub struct MmapFlags(u64);

impl MmapFlags {
    /// MAP_ANONYMOUS: no backing file.
    pub const ANONYMOUS: Self = Self(0x10);
    /// MAP_FIXED: require exact address.
    pub const FIXED: Self = Self(0x100);
    /// MAP_POPULATE: pre-fault pages.
    pub const POPULATE: Self = Self(0x200);
    /// MAP_PRIVATE: copy-on-write.
    pub const PRIVATE: Self = Self(0x02);
    /// MAP_SHARED: share modifications.
    pub const SHARED: Self = Self(0x01);

    /// Construct from a raw u64.

    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// Return the raw u64 value.

    pub const fn raw(self) -> u64 {
        self.0
    }

    /// Check if a flag is set.

    pub const fn contains(self, flag: Self) -> bool {
        self.0 & flag.0 != 0
    }
}

#[derive(Clone)]
/// Backing file for a memory-mapped region.
pub struct FileBacking {
    /// The filesystem containing the file.
    pub fs: Arc<dyn crate::fs::vfs::FileSystem>,
    /// Inode number of the file.
    pub ino: u64,
    /// Offset within the file for this mapping.
    pub file_offset: u64,
    /// Total size of the backing file.
    pub file_size: u64,
    /// mmap flags for this mapping.
    pub mmap_flags: MmapFlags,
    /// Pages that have been written to (for msync).
    pub dirty_pages: Vec<u64>,
}

#[derive(Clone)]
pub struct VmaRegion {
    /// Start virtual address (page-aligned).
    pub start: u64,
    /// Size in bytes.
    pub size: u64,
    /// Access permissions.
    pub flags: VmaFlags,
    /// Type of region.
    pub kind: VmaType,
    /// Optional backing file for mmap.
    pub backing: Option<FileBacking>,
}

impl core::fmt::Debug for VmaRegion {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("VmaRegion")
            .field("start", &self.start)
            .field("size", &self.size)
            .field("kind", &self.kind)
            .field("has_backing", &self.backing.is_some())
            .finish()
    }
}

impl VmaRegion {
    pub fn new_anon(start: u64, size: u64, flags: VmaFlags) -> Self {
        Self {
            start,
            size,
            flags,
            kind: VmaType::Mmap,
            backing: None,
        }
    }

    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    pub fn new_file(
        start: u64,
        size: u64,
        flags: VmaFlags,
        fs: Arc<dyn crate::fs::vfs::FileSystem>,
        ino: u64,
        file_offset: u64,
        file_size: u64,
        mmap_flags: MmapFlags,
    ) -> Self {
        let num_pages = (size + PAGE_SIZE_U64 - 1) / PAGE_SIZE_U64;
        let bitmap_len = (num_pages + 63) / 64;
        let dirty_pages = vec![0u64; bitmap_len as usize];
        Self {
            start,
            size,
            flags,
            kind: VmaType::FileMmap,
            backing: Some(FileBacking {
                fs,
                ino,
                file_offset,
                file_size,
                mmap_flags,
                dirty_pages,
            }),
        }
    }

    pub fn mark_dirty(&mut self, page_offset: u64) {
        if let Some(ref mut backing) = self.backing {
            let page_idx = page_offset / PAGE_SIZE_U64;
            let word = (page_idx / 64) as usize;
            let bit = (page_idx % 64) as u64;
            if word < backing.dirty_pages.len() {
                backing.dirty_pages[word] |= 1u64 << bit;
            }
        }
    }

    pub fn is_dirty(&self, page_offset: u64) -> bool {
        self.backing.as_ref().is_some_and(|backing| {
            let page_idx = page_offset / PAGE_SIZE_U64;
            let word = (page_idx / 64) as usize;
            let bit = (page_idx % 64) as u64;
            word < backing.dirty_pages.len() && backing.dirty_pages[word] & (1u64 << bit) != 0
        })
    }

    pub fn file_offset_for_page(&self, virt: u64) -> Option<u64> {
        self.backing.as_ref().map(|backing| {
            let page_offset = virt - self.start;
            backing.file_offset + page_offset
        })
    }

    pub fn end(&self) -> u64 {
        self.start + self.size
    }

    pub fn num_pages(&self) -> u64 {
        (self.size + PAGE_SIZE_U64 - 1) / PAGE_SIZE_U64
    }

    pub fn contains(&self, addr: u64) -> bool {
        addr >= self.start && addr < self.end()
    }

    pub fn contains_range(&self, addr: u64, len: u64) -> bool {
        addr >= self.start && addr.saturating_add(len) <= self.end()
    }
}

pub struct VmaList {
    regions: Vec<VmaRegion>,
}

impl VmaList {
    pub fn new() -> Self {
        Self {
            regions: Vec::new(),
        }
    }

    pub fn add(&mut self, region: VmaRegion) -> Result<(), &'static str> {
        for existing in &self.regions {
            if region.start < existing.end() && existing.start < region.end() {
                return Err("VMA region overlaps with existing region");
            }
        }
        self.regions.push(region);
        Ok(())
    }

    pub fn remove(&mut self, addr: u64) -> bool {
        if let Some(pos) = self.regions.iter().position(|r| r.start == addr) {
            self.regions.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn find(&self, addr: u64) -> Option<&VmaRegion> {
        self.regions.iter().find(|r| r.contains(addr))
    }

    pub fn find_mut(&mut self, addr: u64) -> Option<&mut VmaRegion> {
        self.regions.iter_mut().find(|r| r.contains(addr))
    }

    pub fn is_writable(&self, addr: u64) -> bool {
        self.find(addr).is_some_and(|r| r.flags.write)
    }

    pub fn is_executable(&self, addr: u64) -> bool {
        self.find(addr).is_some_and(|r| r.flags.execute)
    }

    pub fn validate_access(&self, addr: u64, len: u64, write: bool, execute: bool) -> bool {
        if len == 0 {
            return true;
        }
        let mut cur = addr & !(PAGE_SIZE - 1);
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
                return false;
            }
            cur += PAGE_SIZE;
        }
        true
    }

    pub fn len(&self) -> usize {
        self.regions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    pub fn iter(&self) -> core::slice::Iter<'_, VmaRegion> {
        self.regions.iter()
    }

    pub fn iter_mut(&mut self) -> core::slice::IterMut<'_, VmaRegion> {
        self.regions.iter_mut()
    }

    pub fn extend_heap(&mut self, new_end: u64) -> Option<u64> {
        for region in &mut self.regions {
            if region.kind == VmaType::Heap {
                let old_end = region.end();
                let aligned = (new_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
                if aligned > old_end {
                    region.size = aligned - region.start;
                } else if aligned < region.start {
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

    fn r(s: u64, sz: u64, k: VmaType) -> VmaRegion {
        VmaRegion {
            start: s,
            size: sz,
            flags: VmaFlags::RW,
            kind: k,
            backing: None,
        }
    }

    #[test]
    fn test_vma_region_contains() {
        let r = r(0x1000, 0x2000, VmaType::Data);
        assert!(r.contains(0x1000));
        assert!(r.contains(0x2FFF));
        assert!(!r.contains(0x3000));
        assert!(!r.contains(0x0FFF));
    }

    #[test]
    fn test_vma_region_contains_range() {
        let r = r(0x1000, 0x2000, VmaType::Data);
        assert!(r.contains_range(0x1000, 0x2000));
        assert!(r.contains_range(0x1500, 0x1000));
        assert!(!r.contains_range(0x1000, 0x3000));
    }

    #[test]
    fn test_vma_list_add_and_find() {
        let mut list = VmaList::new();
        list.add(r(0x1000, 0x1000, VmaType::Code)).unwrap();
        list.add(r(0x3000, 0x1000, VmaType::Data)).unwrap();
        assert!(list.find(0x1000).is_some());
        assert!(list.find(0x3000).is_some());
    }

    #[test]
    fn test_vma_list_overlap_rejected() {
        let mut list = VmaList::new();
        list.add(r(0x1000, 0x2000, VmaType::Data)).unwrap();
        assert!(list.add(r(0x2000, 0x1000, VmaType::Heap)).is_err());
    }

    #[test]
    fn test_vma_list_remove() {
        let mut list = VmaList::new();
        list.add(r(0x1000, 0x1000, VmaType::Data)).unwrap();
        assert!(list.remove(0x1000));
        assert!(list.find(0x1000).is_none());
    }

    #[test]
    fn test_vma_validate_access() {
        let mut list = VmaList::new();
        list.add(VmaRegion {
            start: 0x1000,
            size: 0x2000,
            flags: VmaFlags::RX,
            kind: VmaType::Code,
            backing: None,
        })
        .unwrap();
        assert!(list.validate_access(0x1000, 0x100, false, false));
        assert!(!list.validate_access(0x1000, 0x100, true, false));
        assert!(!list.validate_access(0x5000, 0x100, false, false));
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
    fn test_vma_region_new_anon() {
        let r = VmaRegion::new_anon(0x10000, 0x2000, VmaFlags::RW);
        assert_eq!(r.kind, VmaType::Mmap);
        assert!(r.backing.is_none());
    }

    #[test]
    fn test_vma_region_num_pages() {
        let r = VmaRegion {
            start: 0x1000,
            size: 0x3000,
            flags: VmaFlags::RW,
            kind: VmaType::Mmap,
            backing: None,
        };
        assert_eq!(r.num_pages(), 3);
    }

    #[test]
    fn test_vma_list_find_mut() {
        let mut list = VmaList::new();
        list.add(r(0x1000, 0x1000, VmaType::Data)).unwrap();
        assert!(list.find_mut(0x1500).is_some());
        assert!(list.find_mut(0x5000).is_none());
    }

    #[test]
    fn test_mmap_flags() {
        assert!(MmapFlags::SHARED.contains(MmapFlags::SHARED));
        assert!(!MmapFlags::SHARED.contains(MmapFlags::PRIVATE));
        assert!(MmapFlags::ANONYMOUS.contains(MmapFlags::ANONYMOUS));
        let c = MmapFlags::from_raw(0x03);
        assert!(c.contains(MmapFlags::SHARED));
        assert!(c.contains(MmapFlags::PRIVATE));
    }

    #[test]
    fn test_vma_list_len() {
        let mut list = VmaList::new();
        assert!(list.is_empty());
        list.add(r(0x1000, 0x1000, VmaType::Data)).unwrap();
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn test_vma_validate_zero_length() {
        assert!(VmaList::new().validate_access(0x5000, 0, false, false));
    }

    #[test]
    fn test_vma_extend_heap() {
        let mut list = VmaList::new();
        list.add(r(0x5000, 0x1000, VmaType::Heap)).unwrap();
        assert_eq!(list.extend_heap(0x7000), Some(0x6000));
        assert_eq!(list.find(0x5000).unwrap().size, 0x2000);
    }
}
