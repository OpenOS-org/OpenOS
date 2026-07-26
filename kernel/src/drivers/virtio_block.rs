//! VirtIO-Block device driver.
//!
//! Implements the virtio-blk PCI device for reading and writing disk
//! sectors. This is the kernel-side raw block driver; the filesystem
//! runs in user-space.
//!
//! ## Architecture
//!
//! ```text
//! User-space (filesystem)
//!     │
//!     │ channel_send / channel_receive
//!     ▼
//! Kernel driver (this module)
//!     │
//!     │ virtqueue read/write
//!     ▼
//! virtio-blk PCI device
//!     │
//!     │ Block I/O
//!     ▼
//! Disk (QEMU virtual disk)
//! ```
//!
//! ## Legacy `VirtIO` I/O Port Interface
//!
//! This driver uses the legacy (transitional) virtio I/O port interface
//! because QEMU's `virtio-blk-pci` defaults to legacy mode when the
//! transport is PCI. The virtqueue uses the "split virtqueue" layout:
//!
//! - Descriptor Table: array of `(addr, len, flags, next)` entries
//! - Available Ring: driver -> device (which descriptors are ready)
//! - Used Ring: device -> driver (which descriptors the device consumed)
//!
//! ## Request Format
//!
//! Each block I/O request consists of a 3-descriptor chain:
//!   1. Descriptor 0 (device-readable): `VirtioBlkReqHeader` (16 bytes)
//!   2. Descriptor 1 (device-readable for write, device-writable for read):
//!      512 bytes of sector data
//!   3. Descriptor 2 (device-writable): 1-byte status code

use core::sync::atomic::{fence, Ordering};

use spin::Mutex;

use super::pci::{self, PciDevice};

// ---------------------------------------------------------------------------
// VirtIO-Block feature bits
// ---------------------------------------------------------------------------

/// Device reports a capacity in the config space.
const VIRTIO_BLK_F_SIZE_MAX: u64 = 1 << 1;
/// Device supports multi-sector requests.
const VIRTIO_BLK_F_SEG_MAX: u64 = 1 << 2;
/// Geometry is available in the config space.
const VIRTIO_BLK_F_GEOMETRY: u64 = 1 << 4;
/// Device is read-only.
const VIRTIO_BLK_F_RO: u64 = 5;
/// Block size of 512 is always assumed in legacy mode.
const VIRTIO_BLK_F_BLK_SIZE: u64 = 1 << 6;

// ---------------------------------------------------------------------------
// Virtqueue geometry
// ---------------------------------------------------------------------------

/// Number of descriptors per virtqueue (must be a power of two).
const VQ_SIZE: usize = 16;

/// Sector size in bytes.
const SECTOR_SIZE: usize = 512;

/// Page size used by legacy virtio for queue alignment.
const PAGE_SIZE: u64 = 4096;

// ---------------------------------------------------------------------------
// Legacy virtio I/O port register offsets
//
// From the VirtIO 1.0 spec, Appendix B (Legacy Interface):
// ---------------------------------------------------------------------------

/// Device feature bits (read, 32-bit).
const VIRTIO_REG_GUEST_FEATURES: u16 = 0x04;
/// Queue PFN — page frame number of the virtqueue (write, 32-bit).
const VIRTIO_REG_QUEUE_PFN: u16 = 0x08;
/// Queue size — number of elements (read, 16-bit).
const VIRTIO_REG_QUEUE_NUM: u16 = 0x0C;
/// Queue select — which queue to configure (write, 16-bit).
const VIRTIO_REG_QUEUE_SEL: u16 = 0x0E;
/// Queue notify — write queue index to kick the device (write, 16-bit).
const VIRTIO_REG_QUEUE_NOTIFY: u16 = 0x10;
/// Device status (write/read, 8-bit).
const VIRTIO_REG_DEVICE_STATUS: u16 = 0x12;
/// ISR status (read, 8-bit) — reading acknowledges the interrupt.
const VIRTIO_REG_ISR_STATUS: u16 = 0x13;

// ---------------------------------------------------------------------------
// VirtIO device status flags
// ---------------------------------------------------------------------------

/// Indicates that the guest has found the device.
const STATUS_ACKNOWLEDGE: u8 = 1;
/// Indicates that the guest can drive the device.
const STATUS_DRIVER: u8 = 2;
/// Indicates that the driver is set up and ready.
const STATUS_DRIVER_OK: u8 = 4;
/// Indicates that the driver has finished feature negotiation.
const STATUS_FEATURES_OK: u8 = 8;
/// Indicates a fatal error.
const STATUS_FAILED: u8 = 128;

// ---------------------------------------------------------------------------
// Virtqueue descriptor flags
// ---------------------------------------------------------------------------

/// Descriptor continues via `next` field.
const DESC_F_NEXT: u16 = 1;
/// Buffer is device-writable (otherwise device-readable).
const DESC_F_WRITE: u16 = 2;

// ---------------------------------------------------------------------------
// VirtIO-Block request types
// ---------------------------------------------------------------------------

/// Read from the block device.
const VIRTIO_BLK_T_IN: u32 = 0;
/// Write to the block device.
const VIRTIO_BLK_T_OUT: u32 = 1;

/// Status byte: success.
const VIRTIO_BLK_S_OK: u8 = 0;

// ---------------------------------------------------------------------------
// VirtIO-Block request header (16 bytes)
//
// This is the device-readable part of every block I/O request.
// ---------------------------------------------------------------------------

/// VirtIO-Block request header, placed as the first descriptor.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct VirtioBlkReqHeader {
    /// Request type (`VIRTIO_BLK_T_IN` or `VIRTIO_BLK_T_OUT`).
    request_type: u32,
    /// Reserved — must be zero.
    reserved: u32,
    /// Starting sector (LBA).
    sector: u64,
}

// ---------------------------------------------------------------------------
// VirtIO-Block config space (legacy, at I/O base + 0x14)
//
// The config space contains the disk geometry and capacity.
// In legacy mode, only `capacity` (8 bytes at offset 0x14) is guaranteed.
// ---------------------------------------------------------------------------

/// Offset from the I/O base to the block config space.
const BLK_CONFIG_OFFSET: u16 = 0x14;

// ---------------------------------------------------------------------------
// Split Virtqueue structures (placed in memory the device can DMA)
// ---------------------------------------------------------------------------

/// A single descriptor in the virtqueue descriptor table.
///
/// The device reads `addr` as a physical address. `len` is the buffer
/// length. `flags` control chaining and read/write direction. `next`
/// is the index of the next descriptor in a chain.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct VirtqDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

/// Available ring — driver writes, device reads.
///
/// `idx` is the next slot the driver will write. The device reads
/// descriptors from `ring[last_seen_idx .. idx]`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct VirtqAvail {
    flags: u16,
    idx: u16,
    ring: [u16; VQ_SIZE],
}

/// A single element in the used ring.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct VirtqUsedElem {
    id: u32,
    len: u32,
}

/// Used ring — device writes, driver reads.
///
/// `idx` is the next slot the device will write. The driver reads
/// descriptors from `ring[last_consumed_idx .. idx]`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct VirtqUsed {
    flags: u16,
    idx: u16,
    ring: [VirtqUsedElem; VQ_SIZE],
}

// ---------------------------------------------------------------------------
// VirtQueue — runtime state for one virtqueue
// ---------------------------------------------------------------------------

/// Per-queue runtime state.
///
/// The `descriptors`, `avail`, and `used` arrays live at physical
/// addresses whose PFN was written to the device via `QUEUE_PFN`.
/// The device DMAs directly into these structures.
struct VirtQueue {
    /// Descriptor table — device reads these for buffer addresses.
    descriptors: &'static mut [VirtqDesc],
    /// Available ring — driver writes descriptor indices here.
    avail: &'static mut VirtqAvail,
    /// Used ring — device writes completed descriptor indices here.
    used: &'static mut VirtqUsed,

    /// Physical address of the descriptor table.
    desc_phys: u64,
    /// Physical address of the available ring.
    avail_phys: u64,
    /// Physical address of the used ring.
    used_phys: u64,

    /// DMA buffers (physical memory). Each slot holds a 4 KiB frame.
    /// We use individual frames for headers, data, and status bytes.
    buffers: &'static mut [u8],
    /// Physical address of the buffers frame.
    buf_phys: u64,

    /// Free descriptor list — indices of descriptors not currently in use.
    free_head: u16,
    /// Number of free descriptors.
    num_free: u16,
    /// Index of the next descriptor to allocate (round-robin fallback).
    next_alloc: u16,

    /// Last used ring index we consumed. When `used.idx != next_used`,
    /// the device has completed one or more buffers.
    next_used: u16,
}

// ---------------------------------------------------------------------------
// Driver state
// ---------------------------------------------------------------------------

/// VirtIO-Block driver state.
struct VirtioBlockDriver {
    /// PCI device info — kept for debugging and potential reset.
    #[allow(dead_code)]
    pci: PciDevice,
    /// I/O port base address from BAR0.
    io_base: u16,
    /// Total number of sectors on the device.
    sector_count: u64,
    /// Virtqueue (queue 0 — the only queue for block devices).
    queue: VirtQueue,
}

/// Global driver state, protected by a spin lock.
static DRIVER: Mutex<Option<VirtioBlockDriver>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Port I/O helpers
// ---------------------------------------------------------------------------

/// Read a 32-bit value from a virtio I/O port register.
///
/// # Safety
/// `base` must be the I/O port base of a valid virtio device.
/// `offset` must be a valid legacy virtio register offset.
unsafe fn io_read32(base: u16, offset: u16) -> u32 {
    // SAFETY: Caller guarantees `base + offset` is a valid virtio I/O port.
    unsafe {
        let mut port = x86_64::instructions::port::Port::new(base + offset);
        port.read()
    }
}

/// Write a 32-bit value to a virtio I/O port register.
///
/// # Safety
/// `base` must be the I/O port base of a valid virtio device.
/// `offset` must be a valid legacy virtio register offset.
unsafe fn io_write32(base: u16, offset: u16, value: u32) {
    // SAFETY: Caller guarantees `base + offset` is a valid virtio I/O port.
    unsafe {
        let mut port = x86_64::instructions::port::Port::new(base + offset);
        port.write(value);
    }
}

/// Read a 16-bit value from a virtio I/O port register.
///
/// # Safety
/// `base` must be the I/O port base of a valid virtio device.
unsafe fn io_read16(base: u16, offset: u16) -> u16 {
    // SAFETY: Caller guarantees `base + offset` is a valid virtio I/O port.
    unsafe {
        let mut port = x86_64::instructions::port::Port::new(base + offset);
        port.read()
    }
}

/// Write a 16-bit value to a virtio I/O port register.
///
/// # Safety
/// `base` must be the I/O port base of a valid virtio device.
unsafe fn io_write16(base: u16, offset: u16, value: u16) {
    // SAFETY: Caller guarantees `base + offset` is a valid virtio I/O port.
    unsafe {
        let mut port = x86_64::instructions::port::Port::new(base + offset);
        port.write(value);
    }
}

/// Read an 8-bit value from a virtio I/O port register.
///
/// # Safety
/// `base` must be the I/O port base of a valid virtio device.
unsafe fn io_read8(base: u16, offset: u16) -> u8 {
    // SAFETY: Caller guarantees `base + offset` is a valid virtio I/O port.
    unsafe {
        let mut port = x86_64::instructions::port::Port::new(base + offset);
        port.read()
    }
}

/// Write an 8-bit value to a virtio I/O port register.
///
/// # Safety
/// `base` must be the I/O port base of a valid virtio device.
unsafe fn io_write8(base: u16, offset: u16, value: u8) {
    // SAFETY: Caller guarantees `base + offset` is a valid virtio I/O port.
    unsafe {
        let mut port = x86_64::instructions::port::Port::new(base + offset);
        port.write(value);
    }
}

/// Read a 64-bit value from two consecutive 32-bit I/O port registers.
///
/// # Safety
/// `base` must be the I/O port base of a valid virtio device.
/// Reads `offset` (low 32 bits) and `offset + 4` (high 32 bits).
unsafe fn io_read64(base: u16, offset: u16) -> u64 {
    let lo = unsafe { io_read32(base, offset) };
    let hi = unsafe { io_read32(base, offset + 4) };
    u64::from(lo) | (u64::from(hi) << 32)
}

// ---------------------------------------------------------------------------
// Physical address helpers
// ---------------------------------------------------------------------------

/// Convert a virtual address to a physical address.
///
/// Uses the bootloader's `physical_memory_offset` which maps
/// `physical -> virtual` as `virtual = physical + offset`.
/// Therefore `physical = virtual - offset`.
///
/// # Panics
/// Panics if `physical_memory_offset` has not been set (called before boot).
fn virt_to_phys(virt: u64) -> u64 {
    let offset = crate::memory::physical_memory_offset();
    assert!(
        offset != 0,
        "physical_memory_offset not set — virt_to_phys called too early"
    );
    virt.checked_sub(offset)
        .expect("virt_to_phys underflow: virtual address is below physical_memory_offset")
}

// ---------------------------------------------------------------------------
// VirtQueue implementation
// ---------------------------------------------------------------------------

impl VirtQueue {
    /// Create and initialize a new virtqueue.
    ///
    /// All structures are allocated from physical memory via `alloc_frame`
    /// so the device can DMA into them using physical addresses.
    fn new() -> Self {
        // Allocate descriptor table (16 entries x 16 bytes = 256 bytes, fits in 1 frame).
        let desc_phys =
            crate::frame_alloc::alloc_frame().expect("out of frames for virtqueue descriptors");
        let desc_virt = crate::memory::phys_to_virt(desc_phys);
        // SAFETY: Frame is exclusively allocated.
        let descriptors = unsafe {
            core::ptr::write_bytes(desc_virt as *mut u8, 0, 4096);
            core::slice::from_raw_parts_mut(desc_virt as *mut VirtqDesc, VQ_SIZE)
        };
        // Set up free descriptor chain.
        for (i, desc) in descriptors.iter_mut().enumerate().take(VQ_SIZE - 1) {
            desc.next = (i + 1) as u16;
        }
        descriptors[VQ_SIZE - 1].next = 0xFFFF;

        // Allocate available ring (struct on physical frame).
        let avail_phys =
            crate::frame_alloc::alloc_frame().expect("out of frames for virtqueue avail ring");
        let avail_virt = crate::memory::phys_to_virt(avail_phys);
        // SAFETY: Frame is exclusively allocated.
        let avail: &'static mut VirtqAvail = unsafe {
            core::ptr::write_bytes(avail_virt as *mut u8, 0, 4096);
            &mut *(avail_virt as *mut VirtqAvail)
        };

        // Allocate used ring (struct on physical frame).
        let used_phys =
            crate::frame_alloc::alloc_frame().expect("out of frames for virtqueue used ring");
        let used_virt = crate::memory::phys_to_virt(used_phys);
        // SAFETY: Frame is exclusively allocated.
        let used: &'static mut VirtqUsed = unsafe {
            core::ptr::write_bytes(used_virt as *mut u8, 0, 4096);
            &mut *(used_virt as *mut VirtqUsed)
        };

        // Allocate a single 4 KiB buffer frame for request headers, data, and status.
        let buf_phys =
            crate::frame_alloc::alloc_frame().expect("out of frames for virtio-blk buffers");
        let buf_virt = crate::memory::phys_to_virt(buf_phys);
        // SAFETY: Frame is exclusively allocated, zeroed.
        let buffers = unsafe {
            core::ptr::write_bytes(buf_virt as *mut u8, 0, 4096);
            core::slice::from_raw_parts_mut(buf_virt as *mut u8, 4096)
        };

        Self {
            descriptors,
            avail,
            used,
            desc_phys,
            avail_phys,
            used_phys,
            buffers,
            buf_phys,
            free_head: 0,
            num_free: VQ_SIZE as u16,
            next_alloc: 0,
            next_used: 0,
        }
    }

    /// Configure this virtqueue on the device via legacy I/O ports.
    ///
    /// Writes the queue's page frame number to `QUEUE_PFN`, which tells
    /// the device where the descriptor table, available ring, and used
    /// ring live in physical memory.
    ///
    /// # Safety
    /// `io_base` must be a valid virtio I/O port base.
    unsafe fn enable_on_device(&self, io_base: u16, queue_index: u16) {
        // SAFETY: Writing to legacy virtio I/O port registers.
        // 1. Select the queue.
        unsafe {
            io_write16(io_base, VIRTIO_REG_QUEUE_SEL, queue_index);
        }
        // 2. Read back the queue size the device reports.
        let device_queue_size = unsafe { io_read16(io_base, VIRTIO_REG_QUEUE_NUM) };
        crate::serial_println!(
            "[BLK]   Queue {}: device reports size {}",
            queue_index,
            device_queue_size
        );

        // 3. Write the page frame number of the descriptor table.
        let pfn = (self.desc_phys / PAGE_SIZE) as u32;
        // SAFETY: Writing queue PFN to valid legacy virtio register.
        unsafe {
            io_write32(io_base, VIRTIO_REG_QUEUE_PFN, pfn);
        }

        crate::serial_println!(
            "[BLK]   Queue {}: PFN={:#x} (desc_phys={:#x})",
            queue_index,
            pfn,
            self.desc_phys
        );
    }

    /// Allocate a free descriptor index.
    ///
    /// Returns `None` if all descriptors are in use (the device has not
    /// yet consumed enough buffers).
    fn alloc_desc(&mut self) -> Option<u16> {
        if self.num_free == 0 {
            return None;
        }
        let idx = self.free_head;
        // Advance the free head to the next free descriptor.
        self.free_head = self.descriptors[idx as usize].next;
        self.num_free -= 1;
        // Mark this descriptor as end-of-chain (no next).
        self.descriptors[idx as usize].next = 0xFFFF;
        Some(idx)
    }

    /// Free a descriptor, returning it to the free list.
    ///
    /// # Safety
    /// `idx` must be a valid descriptor index that was previously
    /// allocated and not yet freed.
    fn free_desc(&mut self, idx: u16) {
        self.descriptors[idx as usize].next = self.free_head;
        self.free_head = idx;
        self.num_free += 1;
    }

    /// Submit a descriptor to the available ring and notify the device.
    ///
    /// This is the final step of the block I/O path: the descriptor
    /// chain has been filled in, now we tell the device about it.
    ///
    /// # Safety
    /// `io_base` must be a valid virtio I/O port base.
    unsafe fn submit_and_notify(&mut self, desc_idx: u16, io_base: u16, queue_index: u16) {
        // Write descriptor index into the available ring at position `avail.idx`.
        let slot = self.avail.idx as usize % VQ_SIZE;
        self.avail.ring[slot] = desc_idx;
        // Memory barrier: ensure the descriptor and ring writes are visible
        // before we update the index.
        fence(Ordering::Release);
        self.avail.idx = self.avail.idx.wrapping_add(1);

        // Notify the device by writing the queue index to QUEUE_NOTIFY.
        // SAFETY: Writing to valid legacy virtio notify register.
        unsafe {
            io_write16(io_base, VIRTIO_REG_QUEUE_NOTIFY, queue_index);
        }
    }

    /// Check the used ring for completed descriptors.
    ///
    /// Returns `Some((desc_id, len))` if the device has completed a
    /// buffer, or `None` if nothing new is available.
    fn poll_used(&mut self) -> Option<(u16, u32)> {
        // Memory barrier: ensure we read the device's latest write to used.idx.
        fence(Ordering::Acquire);
        if self.next_used == self.used.idx {
            return None;
        }
        let slot = self.next_used as usize % VQ_SIZE;
        let elem = self.used.ring[slot];
        self.next_used = self.next_used.wrapping_add(1);
        Some((elem.id as u16, elem.len))
    }
}

// ---------------------------------------------------------------------------
// VirtIO-Block driver initialization
// ---------------------------------------------------------------------------

/// Initialize the VirtIO-Block driver.
///
/// Scans the PCI bus for a virtio-blk device, negotiates features,
/// sets up the virtqueue, reads the disk capacity, and marks the
/// device as `DRIVER_OK`.
///
/// Returns `true` if the device was found and initialized successfully.
#[allow(clippy::too_many_lines)]
pub fn init() -> bool {
    crate::serial_println!("[BLK] Scanning PCI bus for virtio-blk...");

    let Some(dev) = pci::find_device(pci::VIRTIO_VENDOR_ID, pci::VIRTIO_BLK_DEVICE_ID) else {
        crate::serial_println!("[BLK] No virtio-blk device found");
        return false;
    };

    crate::serial_println!(
        "[BLK] Found virtio-blk at PCI {}:{}:{}, BAR0={:#x}",
        dev.bus,
        dev.device,
        dev.function,
        dev.bar0
    );

    // Determine device access method from BAR0.
    // BAR0 bit 0: 0 = MMIO, 1 = I/O port.
    let io_base = if dev.bar0 & 1 != 0 {
        // I/O port BAR — mask off the type bits.
        (dev.bar0 & 0xFFFC) as u16
    } else {
        // MMIO BAR — not supported in this driver. We require I/O port.
        crate::serial_println!("[BLK] ERROR: MMIO BAR not supported, need I/O port BAR");
        return false;
    };

    crate::serial_println!("[BLK] I/O base: {:#x}", io_base);

    // ---------------------------------------------------------------
    // Legacy virtio initialization sequence (spec Section 3.1.1):
    //   1. Reset device (status = 0)
    //   2. Set ACKNOWLEDGE
    //   3. Set DRIVER
    //   4. Read and negotiate feature bits
    //   5. Set FEATURES_OK
    //   6. Set up virtqueue
    //   7. Set DRIVER_OK
    // ---------------------------------------------------------------

    // Step 1: Reset — write 0 to device status.
    // SAFETY: Writing to valid virtio device status register.
    unsafe {
        io_write8(io_base, VIRTIO_REG_DEVICE_STATUS, 0);
    }

    // Step 2: Acknowledge the device.
    // SAFETY: Writing to valid virtio device status register.
    unsafe {
        io_write8(io_base, VIRTIO_REG_DEVICE_STATUS, STATUS_ACKNOWLEDGE);
    }

    // Step 3: Indicate we have a driver for this device.
    // SAFETY: Writing to valid virtio device status register.
    unsafe {
        io_write8(
            io_base,
            VIRTIO_REG_DEVICE_STATUS,
            STATUS_ACKNOWLEDGE | STATUS_DRIVER,
        );
    }

    // Step 4: Feature negotiation.
    // Read device features and accept only what we support.
    // SAFETY: Reading from valid virtio feature register.
    let device_features = unsafe { io_read32(io_base, VIRTIO_REG_GUEST_FEATURES) };
    // We negotiate minimal features for a basic block device.
    let guest_features = device_features as u64 & (VIRTIO_BLK_F_SIZE_MAX | VIRTIO_BLK_F_BLK_SIZE);
    // SAFETY: Writing to valid virtio feature register.
    unsafe {
        io_write32(io_base, VIRTIO_REG_GUEST_FEATURES, guest_features as u32);
    }

    crate::serial_println!(
        "[BLK] Features: device={:#x}, negotiated={:#x}",
        device_features,
        guest_features
    );

    // Step 5: Features OK.
    // SAFETY: Writing to valid virtio device status register.
    unsafe {
        io_write8(
            io_base,
            VIRTIO_REG_DEVICE_STATUS,
            STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK,
        );
    }
    // Verify the device accepted our features.
    // SAFETY: Reading from valid virtio device status register.
    let status = unsafe { io_read8(io_base, VIRTIO_REG_DEVICE_STATUS) };
    if status & STATUS_FEATURES_OK == 0 {
        crate::serial_println!("[BLK] ERROR: device rejected feature negotiation");
        // Set FAILED status.
        // SAFETY: Writing to valid virtio device status register.
        unsafe {
            io_write8(io_base, VIRTIO_REG_DEVICE_STATUS, STATUS_FAILED);
        }
        return false;
    }

    // Step 6: Read disk capacity from device config space.
    // In legacy mode, the capacity (in 512-byte sectors) is at I/O base + 0x14.
    let sector_count = unsafe { io_read64(io_base, BLK_CONFIG_OFFSET) };
    let size_mb = (sector_count * SECTOR_SIZE as u64) / (1024 * 1024);
    crate::serial_println!("[BLK] Capacity: {} sectors ({} MiB)", sector_count, size_mb);

    // Step 7: Set up virtqueue (queue 0 — the only queue for block devices).
    let queue = VirtQueue::new();

    crate::serial_println!("[BLK] Setting up queue (0)...");
    // SAFETY: I/O base is valid — we're in the init path after PCI probe.
    unsafe {
        queue.enable_on_device(io_base, 0);
    }

    // Step 8: Set DRIVER_OK — the device is now live.
    // SAFETY: Writing to valid virtio device status register.
    unsafe {
        io_write8(
            io_base,
            VIRTIO_REG_DEVICE_STATUS,
            STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK,
        );
    }

    let driver = VirtioBlockDriver {
        pci: dev,
        io_base,
        sector_count,
        queue,
    };

    *DRIVER.lock() = Some(driver);
    crate::serial_println!("[OK] virtio-blk initialized (legacy I/O port)");
    true
}

// ---------------------------------------------------------------------------
// Block I/O path
// ---------------------------------------------------------------------------

/// Perform a single-sector block I/O request (read or write).
///
/// Allocates 3 descriptors chained together:
///   1. Header (device-readable): request type, reserved, sector
///   2. Data (device-readable for write, device-writable for read): 512 bytes
///   3. Status (device-writable): 1 byte
///
/// Submits the chain to the device and spins until completion.
///
/// # Errors
/// Returns `Err(())` if the driver is not initialized, the queue is full,
/// or the device reports an error.
#[allow(clippy::cast_ptr_alignment)]
fn do_sector_io(lba: u64, data: &mut [u8; SECTOR_SIZE], is_write: bool) -> Result<(), ()> {
    let mut driver = DRIVER.lock();
    let drv = driver.as_mut().ok_or(())?;

    // Allocate 3 descriptors for the chained request.
    let desc_hdr = drv.queue.alloc_desc().ok_or(())?;
    let desc_data = drv.queue.alloc_desc().ok_or_else(|| {
        // Free the header descriptor on failure.
        drv.queue.free_desc(desc_hdr);
    })?;
    let desc_status = drv.queue.alloc_desc().ok_or_else(|| {
        // Free header and data descriptors on failure.
        drv.queue.free_desc(desc_hdr);
        drv.queue.free_desc(desc_data);
    })?;

    // Lay out the request components in the buffer frame.
    // Header: bytes 0..16, Data: bytes 16..528, Status: byte 528.
    let hdr_offset: usize = 0;
    let data_offset: usize = 16;
    let status_offset: usize = data_offset + SECTOR_SIZE;

    // Write the request header.
    let hdr = VirtioBlkReqHeader {
        request_type: if is_write {
            VIRTIO_BLK_T_OUT
        } else {
            VIRTIO_BLK_T_IN
        },
        reserved: 0,
        sector: lba,
    };
    // SAFETY: Writing to exclusively owned buffer frame.
    // The buffer is allocated with DMA-compatible alignment.
    #[allow(clippy::cast_ptr_alignment)]
    {
        let ptr = drv.queue.buffers[hdr_offset..]
            .as_mut_ptr()
            .cast::<VirtioBlkReqHeader>();
        unsafe {
            core::ptr::write(ptr, hdr);
        }
    }

    // For writes, copy the data into the buffer.
    if is_write {
        drv.queue.buffers[data_offset..data_offset + SECTOR_SIZE].copy_from_slice(data);
    }

    // Zero the status byte (device will write the result here).
    drv.queue.buffers[status_offset] = 0;

    // Physical addresses for each buffer segment.
    let hdr_phys = drv.queue.buf_phys + hdr_offset as u64;
    let data_phys = drv.queue.buf_phys + data_offset as u64;
    let status_phys = drv.queue.buf_phys + status_offset as u64;

    // Descriptor 0: header (device-readable).
    drv.queue.descriptors[desc_hdr as usize].addr = hdr_phys;
    drv.queue.descriptors[desc_hdr as usize].len =
        core::mem::size_of::<VirtioBlkReqHeader>() as u32;
    drv.queue.descriptors[desc_hdr as usize].flags = DESC_F_NEXT;
    drv.queue.descriptors[desc_hdr as usize].next = desc_data;

    // Descriptor 1: data (device-readable for write, device-writable for read).
    drv.queue.descriptors[desc_data as usize].addr = data_phys;
    drv.queue.descriptors[desc_data as usize].len = SECTOR_SIZE as u32;
    drv.queue.descriptors[desc_data as usize].flags = if is_write {
        DESC_F_NEXT
    } else {
        DESC_F_NEXT | DESC_F_WRITE
    };
    drv.queue.descriptors[desc_data as usize].next = desc_status;

    // Descriptor 2: status (device-writable — 1 byte).
    drv.queue.descriptors[desc_status as usize].addr = status_phys;
    drv.queue.descriptors[desc_status as usize].len = 1;
    drv.queue.descriptors[desc_status as usize].flags = DESC_F_WRITE;

    // Submit the chain starting at the header descriptor.
    // SAFETY: `io_base` is valid for this initialized driver.
    unsafe {
        drv.queue.submit_and_notify(desc_hdr, drv.io_base, 0);
    }

    // Spin until the device completes the request.
    let io_base = drv.io_base;
    loop {
        fence(Ordering::Acquire);
        if let Some((_used_id, _used_len)) = drv.queue.poll_used() {
            break;
        }
        // Small delay to avoid hammering the bus.
        for _ in 0..100 {
            // SAFETY: Reading from ISR status register (acknowledges interrupt).
            unsafe {
                io_read8(io_base, VIRTIO_REG_ISR_STATUS);
            }
        }
    }

    // Free all 3 descriptors.
    drv.queue.free_desc(desc_hdr);
    drv.queue.free_desc(desc_data);
    drv.queue.free_desc(desc_status);

    // Check the status byte.
    let status_byte = drv.queue.buffers[status_offset];
    if status_byte != VIRTIO_BLK_S_OK {
        crate::serial_println!(
            "[BLK] ERROR: sector {} returned status {}",
            lba,
            status_byte
        );
        return Err(());
    }

    // For reads, copy the data out of the buffer.
    if !is_write {
        data.copy_from_slice(&drv.queue.buffers[data_offset..data_offset + SECTOR_SIZE]);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Read a single 512-byte sector from the block device.
///
/// # Errors
/// Returns `Err(())` if the driver is not initialized or the device
/// reports an error.
pub fn read_sector(lba: u64, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), ()> {
    do_sector_io(lba, buf, false)
}

/// Write a single 512-byte sector to the block device.
///
/// # Errors
/// Returns `Err(())` if the driver is not initialized or the device
/// reports an error.
pub fn write_sector(lba: u64, buf: &[u8; SECTOR_SIZE]) -> Result<(), ()> {
    let mut data = *buf;
    do_sector_io(lba, &mut data, true)
}

/// Get the total number of sectors on the block device.
///
/// Returns 0 if the driver is not initialized.
pub fn sector_count() -> u64 {
    DRIVER.lock().as_ref().map_or(0, |d| d.sector_count)
}
