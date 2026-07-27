//! System V-style shared memory implementation.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

const PAGE_SIZE: u64 = 0x1000;
const MIN_SHM_SIZE: u64 = 1;
const MAX_SHM_SIZE: u64 = 16 * 1024 * 1024;

/// A System V-style shared memory segment.
pub struct SharedMemorySegment {
    /// Unique segment identifier.
    pub id: u32,
    /// User-supplied key for segment lookup.
    pub key: u32,
    /// Aligned size in bytes (multiple of page size).
    pub size: u64,
    /// Number of active attachments.
    pub refcount: u32,
    /// Physical frame addresses backing this segment.
    pub pages: Vec<u64>,
    /// Whether `shmctl(IPC_RMID)` has been called.
    pub marked_for_removal: bool,
}

/// Global table of all shared memory segments.
pub static SHM_TABLE: spin::Mutex<BTreeMap<u32, SharedMemorySegment>> =
    spin::Mutex::new(BTreeMap::new());

static NEXT_ID: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1);

/// Create a new segment if it does not exist.
pub const IPC_CREAT: u32 = 0o1000;
/// Fail if the segment already exists.
pub const IPC_EXCL: u32 = 0o2000;

/// Get or create a shared memory segment.
pub fn shmget(key: u32, size: u64, flags: u32) -> Result<u32, crate::syscall::Error> {
    if !(MIN_SHM_SIZE..=MAX_SHM_SIZE).contains(&size) {
        return Err(crate::syscall::Error::InvalidArgument);
    }
    let create = key == 0 || (flags & IPC_CREAT) != 0;
    let exclusive = (flags & IPC_EXCL) != 0;
    let mut table = SHM_TABLE.lock();
    if key != 0 {
        let existing = table
            .values()
            .find(|seg| seg.key == key && !seg.marked_for_removal);
        if let Some(seg) = existing {
            if exclusive && create {
                return Err(crate::syscall::Error::AlreadyExists);
            }
            return Ok(seg.id);
        }
    }
    if !create {
        return Err(crate::syscall::Error::NotFound);
    }
    let aligned_size = (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let num_pages = aligned_size / PAGE_SIZE;
    let mut pages = Vec::with_capacity(num_pages as usize);
    for _ in 0..num_pages {
        let frame = crate::frame_alloc::alloc_frame().ok_or(crate::syscall::Error::OutOfMemory)?;
        #[cfg(not(test))]
        {
            let virt = crate::memory::phys_to_virt(frame) as *mut u8;
            // SAFETY: frame was just allocated by the frame allocator.
            unsafe {
                core::ptr::write_bytes(virt, 0, PAGE_SIZE as usize);
            }
        }
        pages.push(frame);
    }
    let id = NEXT_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    crate::serial_println!(
        "[SHM] shmget: key={} id={} size={} pages={}",
        key,
        id,
        aligned_size,
        num_pages
    );
    table.insert(
        id,
        SharedMemorySegment {
            id,
            key,
            size: aligned_size,
            refcount: 0,
            pages,
            marked_for_removal: false,
        },
    );
    Ok(id)
}

/// Attach a shared memory segment to the current process address space.
pub fn shmat(segment_id: u32, _flags: u64) -> Result<u64, crate::syscall::Error> {
    let mut table = SHM_TABLE.lock();
    let seg = table
        .get_mut(&segment_id)
        .filter(|s| !s.marked_for_removal)
        .ok_or(crate::syscall::Error::NotFound)?;
    let num_pages = seg.size.div_ceil(PAGE_SIZE);
    let pages = seg.pages.clone();
    let seg_size = seg.size;
    let virt_addr = crate::task::scheduler::with_current_task(|task| {
        let pt = unsafe { crate::memory::pagetable::PageTable::new(task.page_table.unwrap_or(0)) };
        pt.find_free_range(0x7000_0000, num_pages as usize)
    });
    let Some(Some(virt)) = virt_addr else {
        return Err(crate::syscall::Error::OutOfMemory);
    };
    let pt_flags = x86_64::structures::paging::PageTableFlags::PRESENT
        | x86_64::structures::paging::PageTableFlags::USER_ACCESSIBLE
        | x86_64::structures::paging::PageTableFlags::WRITABLE
        | x86_64::structures::paging::PageTableFlags::NO_EXECUTE;
    for (i, &phys) in pages.iter().enumerate() {
        let page_virt = virt + (i as u64) * PAGE_SIZE;
        // SAFETY: phys is a valid allocated frame, page_virt is page-aligned.
        unsafe {
            crate::task::user::map_page_user(page_virt, phys, pt_flags);
        }
    }
    seg.refcount += 1;
    crate::task::scheduler::with_current_task_mut(|task| {
        task.shm_attachments.push(crate::task::task::ShmAttachment {
            shmid: segment_id,
            virt_addr: virt,
            size: seg_size,
        });
    });
    crate::serial_println!(
        "[SHM] shmat: id={} virt={:#x} pages={}",
        segment_id,
        virt,
        num_pages
    );
    Ok(virt)
}

/// Detach a shared memory segment from the current process address space.
pub fn shmdt(virt_addr: u64) -> Result<(), crate::syscall::Error> {
    if virt_addr == 0 || virt_addr % PAGE_SIZE != 0 {
        return Err(crate::syscall::Error::InvalidArgument);
    }
    let attachment = crate::task::scheduler::with_current_task_mut(|task| {
        let idx = task
            .shm_attachments
            .iter()
            .position(|a| a.virt_addr == virt_addr)?;
        Some(task.shm_attachments.remove(idx))
    });
    let Some(Some(att)) = attachment else {
        return Err(crate::syscall::Error::NotFound);
    };
    crate::task::scheduler::with_current_task_mut(|task| {
        task.vma_list.remove(att.virt_addr);
    });
    let num_pages = att.size.div_ceil(PAGE_SIZE);
    for i in 0..num_pages {
        let page_virt = att.virt_addr + i * PAGE_SIZE;
        // SAFETY: unmapping user pages in the current page table.
        unsafe {
            crate::task::user::map_page_user(
                page_virt,
                0,
                x86_64::structures::paging::PageTableFlags::empty(),
            );
        }
    }
    let mut table = SHM_TABLE.lock();
    let mut should_free = false;
    if let Some(seg) = table.get_mut(&att.shmid) {
        seg.refcount = seg.refcount.saturating_sub(1);
        if seg.marked_for_removal && seg.refcount == 0 {
            should_free = true;
        }
    }
    if should_free {
        if let Some(seg) = table.remove(&att.shmid) {
            for &frame in &seg.pages {
                crate::frame_alloc::free_frame(frame);
            }
        }
    }
    Ok(())
}

/// Mark a shared memory segment for removal (`IPC_RMID`).
pub fn shm_mark_removal(shmid: u32) -> Result<(), crate::syscall::Error> {
    let mut table = SHM_TABLE.lock();
    match table.get_mut(&shmid) {
        Some(seg) => {
            seg.marked_for_removal = true;
            if seg.refcount == 0 {
                if let Some(removed) = table.remove(&shmid) {
                    for &frame in &removed.pages {
                        crate::frame_alloc::free_frame(frame);
                    }
                }
            }
            Ok(())
        }
        None => Err(crate::syscall::Error::NotFound),
    }
}

/// Clean up shared memory attachments when a task exits.
pub fn cleanup_task_attachments(attachments: &[crate::task::task::ShmAttachment]) {
    for att in attachments {
        #[cfg(not(test))]
        {
            let num_pages = att.size.div_ceil(PAGE_SIZE);
            for i in 0..num_pages {
                let page_virt = att.virt_addr + i * PAGE_SIZE;
                // SAFETY: unmapping user pages on behalf of a terminating task.
                unsafe {
                    crate::task::user::map_page_user(
                        page_virt,
                        0,
                        x86_64::structures::paging::PageTableFlags::empty(),
                    );
                }
            }
        }
        let mut table = SHM_TABLE.lock();
        let mut should_free = false;
        if let Some(seg) = table.get_mut(&att.shmid) {
            seg.refcount = seg.refcount.saturating_sub(1);
            if seg.marked_for_removal && seg.refcount == 0 {
                should_free = true;
            }
        }
        if should_free {
            if let Some(seg) = table.remove(&att.shmid) {
                for &frame in &seg.pages {
                    crate::frame_alloc::free_frame(frame);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    static TEST_LOCK: spin::Mutex<()> = spin::Mutex::new(());

    #[test]
    fn test_page_size_constant() {
        assert_eq!(PAGE_SIZE, 0x1000);
    }
    #[test]
    fn test_ipc_creat_flag() {
        assert_eq!(IPC_CREAT, 0o1000);
    }
    #[test]
    fn test_ipc_excl_flag() {
        assert_eq!(IPC_EXCL, 0o2000);
    }
    #[test]
    fn test_shm_segment_alignment() {
        assert_eq!((1u64 + PAGE_SIZE - 1) & !(PAGE_SIZE - 1), PAGE_SIZE);
        assert_eq!((PAGE_SIZE + PAGE_SIZE - 1) & !(PAGE_SIZE - 1), PAGE_SIZE);
        assert_eq!(
            (PAGE_SIZE + 1 + PAGE_SIZE - 1) & !(PAGE_SIZE - 1),
            2 * PAGE_SIZE
        );
    }
    #[test]
    fn test_shmget_zero_size_rejected() {
        let _g = TEST_LOCK.lock();
        assert!(shmget(1, 0, IPC_CREAT).is_err());
    }
    #[test]
    fn test_shmget_too_large_size_rejected() {
        let _g = TEST_LOCK.lock();
        assert!(shmget(1, MAX_SHM_SIZE + 1, IPC_CREAT).is_err());
    }
    #[test]
    fn test_shmget_no_create_no_exist() {
        let _g = TEST_LOCK.lock();
        assert!(shmget(99999, 4096, 0).is_err());
    }
    #[test]
    #[ignore] // requires physical_memory_offset (bare-metal only)
    fn test_shmget_exclusive_conflict() {
        let _g = TEST_LOCK.lock();
        crate::frame_alloc::reset();
        {
            SHM_TABLE.lock().clear();
        }
        let id1 = shmget(42, 4096, IPC_CREAT | IPC_EXCL).unwrap();
        assert!(shmget(42, 4096, IPC_CREAT | IPC_EXCL).is_err());
        let _ = shm_mark_removal(id1);
    }
    #[test]
    #[ignore] // requires physical_memory_offset (bare-metal only)
    fn test_shmget_creates_segment_with_valid_id() {
        let _g = TEST_LOCK.lock();
        crate::frame_alloc::reset();
        {
            SHM_TABLE.lock().clear();
        }
        let id = shmget(100, 8192, IPC_CREAT).unwrap();
        assert!(id > 0);
        assert_eq!(id, shmget(100, 8192, 0).unwrap());
        let _ = shm_mark_removal(id);
    }
    #[test]
    #[ignore] // requires physical_memory_offset (bare-metal only)
    fn test_shmget_size_rounded_to_page() {
        let _g = TEST_LOCK.lock();
        crate::frame_alloc::reset();
        {
            SHM_TABLE.lock().clear();
        }
        let id = shmget(200, 1, IPC_CREAT).unwrap();
        let table = SHM_TABLE.lock();
        let seg = table.get(&id).unwrap();
        assert_eq!(seg.pages.len(), 1);
        assert_eq!(seg.size, PAGE_SIZE);
        drop(table);
        let _ = shm_mark_removal(id);
    }
    #[test]
    #[ignore] // requires physical_memory_offset (bare-metal only)
    fn test_shmget_mark_removal_no_refs() {
        let _g = TEST_LOCK.lock();
        crate::frame_alloc::reset();
        {
            SHM_TABLE.lock().clear();
        }
        let id = shmget(300, 4096, IPC_CREAT).unwrap();
        assert_eq!(SHM_TABLE.lock().get(&id).unwrap().refcount, 0);
        shm_mark_removal(id).unwrap();
        assert!(SHM_TABLE.lock().get(&id).is_none());
    }
    #[test]
    #[ignore] // requires physical_memory_offset (bare-metal only)
    fn test_shm_mark_removal_with_active_ref() {
        let _g = TEST_LOCK.lock();
        crate::frame_alloc::reset();
        {
            SHM_TABLE.lock().clear();
        }
        let id = shmget(400, 4096, IPC_CREAT).unwrap();
        {
            SHM_TABLE.lock().get_mut(&id).unwrap().refcount = 1;
        }
        shm_mark_removal(id).unwrap();
        {
            let t = SHM_TABLE.lock();
            let s = t.get(&id).unwrap();
            assert!(s.marked_for_removal);
            assert_eq!(s.refcount, 1);
        }
        {
            let mut t = SHM_TABLE.lock();
            let s = t.get_mut(&id).unwrap();
            s.refcount = 0;
            if s.marked_for_removal && s.refcount == 0 {
                let r = t.remove(&id).unwrap();
                for &f in &r.pages {
                    crate::frame_alloc::free_frame(f);
                }
            }
        }
        assert!(SHM_TABLE.lock().get(&id).is_none());
    }
    #[test]
    #[ignore] // requires physical_memory_offset (bare-metal only)
    fn test_shmget_multiple_segments() {
        let _g = TEST_LOCK.lock();
        crate::frame_alloc::reset();
        {
            SHM_TABLE.lock().clear();
        }
        let id1 = shmget(500, 4096, IPC_CREAT).unwrap();
        let id2 = shmget(501, 8192, IPC_CREAT).unwrap();
        let id3 = shmget(502, 16384, IPC_CREAT).unwrap();
        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        let _ = shm_mark_removal(id1);
        let _ = shm_mark_removal(id2);
        let _ = shm_mark_removal(id3);
    }
    #[test]
    fn test_cleanup_task_attachments_empty() {
        let _g = TEST_LOCK.lock();
        cleanup_task_attachments(&[]);
    }
    #[test]
    #[ignore] // requires physical_memory_offset (bare-metal only)
    fn test_cleanup_task_attachments_with_removal() {
        let _g = TEST_LOCK.lock();
        crate::frame_alloc::reset();
        {
            SHM_TABLE.lock().clear();
        }
        let id = shmget(600, 4096, IPC_CREAT).unwrap();
        {
            let mut t = SHM_TABLE.lock();
            let s = t.get_mut(&id).unwrap();
            s.refcount = 1;
            s.marked_for_removal = true;
        }
        let att = crate::task::task::ShmAttachment {
            shmid: id,
            virt_addr: 0x7000_0000,
            size: 4096,
        };
        cleanup_task_attachments(&[att]);
        assert!(SHM_TABLE.lock().get(&id).is_none());
    }
    #[test]
    #[ignore] // requires physical_memory_offset (bare-metal only)
    fn test_shmget_private_key_always_creates() {
        let _g = TEST_LOCK.lock();
        crate::frame_alloc::reset();
        {
            SHM_TABLE.lock().clear();
        }
        let id1 = shmget(0, 4096, IPC_CREAT).unwrap();
        let id2 = shmget(0, 4096, IPC_CREAT).unwrap();
        assert_ne!(id1, id2);
        let _ = shm_mark_removal(id1);
        let _ = shm_mark_removal(id2);
    }

    #[test]
    fn test_shmdt_null_virt_addr_rejected() {
        assert_eq!(shmdt(0), Err(crate::syscall::Error::InvalidArgument));
    }

    #[test]
    fn test_shmdt_non_aligned_virt_addr_rejected() {
        // 0x7000_0001 is not page-aligned: 0x7000_0001 % 0x1000 == 1.
        assert_eq!(
            shmdt(0x7000_0001),
            Err(crate::syscall::Error::InvalidArgument)
        );
        // 0x8000_0123 is not page-aligned.
        assert_eq!(
            shmdt(0x8000_0123),
            Err(crate::syscall::Error::InvalidArgument)
        );
    }

    #[test]
    fn test_shmdt_page_aligned_but_not_attached() {
        assert_eq!(shmdt(0x7000_0000), Err(crate::syscall::Error::NotFound));
        assert_eq!(shmdt(0x8000_0000), Err(crate::syscall::Error::NotFound));
    }

    #[test]
    fn test_shm_mark_removal_nonexistent() {
        let id = 99999u32;
        assert_eq!(shm_mark_removal(id), Err(crate::syscall::Error::NotFound));
    }

    #[test]
    fn test_next_id_increments() {
        let before = NEXT_ID.load(core::sync::atomic::Ordering::Relaxed);
        let after = before + 1;
        NEXT_ID.store(before, core::sync::atomic::Ordering::Relaxed);
        let _id = NEXT_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        assert_eq!(NEXT_ID.load(core::sync::atomic::Ordering::Relaxed), after);
    }

    #[test]
    fn test_shm_size_alignment_computation() {
        let test_cases = &[
            (1u64, 0x1000u64),
            (0x1000u64, 0x1000u64),
            (0x1001u64, 0x2000u64),
            (0x2000u64, 0x2000u64),
            (0x3000u64, 0x3000u64),
            (0x4001u64, 0x5000u64),
            (16u64 * 1024 * 1024, 16u64 * 1024 * 1024),
        ];
        for &(size, expected) in test_cases {
            let aligned = (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
            assert_eq!(aligned, expected, "size={:#x}", size);
        }
    }

    #[test]
    fn test_shm_min_max_constants() {
        assert_eq!(MIN_SHM_SIZE, 1);
        assert_eq!(MAX_SHM_SIZE, 16 * 1024 * 1024);
    }

    #[test]
    fn test_shm_segment_size_within_bounds() {
        assert!((MIN_SHM_SIZE..=MAX_SHM_SIZE).contains(&(16 * 1024 * 1024)));
        assert!(!(MIN_SHM_SIZE..=MAX_SHM_SIZE).contains(&(16 * 1024 * 1024 + 1)));
    }

    #[test]
    fn test_cleanup_task_attachments_without_removal() {
        let _g = TEST_LOCK.lock();
        let att = crate::task::task::ShmAttachment {
            shmid: 99999,
            virt_addr: 0,
            size: 4096,
        };
        cleanup_task_attachments(&[att]);
    }
}
