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
//! transport is PCI. The virtqueue uses the "split virtqueue" layout
//! (shared types live in [`super::virtio`]).
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
use super::virtio::{
    self, io_read64, io_read8, io_write16, io_write32, io_write8, VirtQueue, DESC_F_NEXT,
    DESC_F_WRITE, PAGE_SIZE, VIRTIO_REG_DEVICE_STATUS, VIRTIO_REG_GUEST_FEATURES,
    VIRTIO_REG_ISR_STATUS, VIRTIO_REG_QUEUE_NOTIFY, VQ_SIZE,
};

// ---------------------------------------------------------------------------
// VirtIO-Block feature bits
// ---------------------------------------------------------------------------

/// Device reports a capacity in the config space.
const VIRTIO_BLK_F_SIZE_MAX: u64 = 1 << 1;
/// Device supports multi-sector requests.
const VIRTIO_BLK_F_SEG_MAX: u64 = 1 << 2;
/// Geometry is available in the config space.
const VIRTIO_BLK_F_GEOMETRY: u64 = 1 << 4;
/// Device is read-only (bit 5).
const VIRTIO_BLK_F_RO: u64 = 1 << 5;
/// Block size of 512 is always assumed in legacy mode.
const VIRTIO_BLK_F_BLK_SIZE: u64 = 1 << 6;

// ---------------------------------------------------------------------------
// Sector size
// ---------------------------------------------------------------------------

/// Sector size in bytes.
const SECTOR_SIZE: usize = 512;

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
    /// Reserved -- must be zero.
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
// Driver state
// ---------------------------------------------------------------------------

/// VirtIO-Block driver state.
struct VirtioBlockDriver {
    /// PCI device info -- kept for debugging and potential reset.
    #[allow(dead_code)]
    pci: PciDevice,
    /// I/O port base address from BAR0.
    io_base: u16,
    /// Total number of sectors on the device.
    sector_count: u64,
    /// Virtqueue (queue 0 -- the only queue for block devices).
    queue: VirtQueue,
    /// DMA buffer frame (physical memory). A single 4 KiB frame used for
    /// request headers, data, and status bytes.
    buffers: &'static mut [u8],
    /// Physical address of the buffer frame.
    buf_phys: u64,
}

/// Global driver state, protected by a spin lock.
static DRIVER: Mutex<Option<VirtioBlockDriver>> = Mutex::new(None);

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
        let ptr = drv.buffers[hdr_offset..]
            .as_mut_ptr()
            .cast::<VirtioBlkReqHeader>();
        unsafe {
            core::ptr::write(ptr, hdr);
        }
    }

    // For writes, copy the data into the buffer.
    if is_write {
        drv.buffers[data_offset..data_offset + SECTOR_SIZE].copy_from_slice(data);
    }

    // Zero the status byte (device will write the result here).
    drv.buffers[status_offset] = 0;

    // Physical addresses for each buffer segment.
    let hdr_phys = drv.buf_phys + hdr_offset as u64;
    let data_phys = drv.buf_phys + data_offset as u64;
    let status_phys = drv.buf_phys + status_offset as u64;

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

    // Descriptor 2: status (device-writable -- 1 byte).
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
                let _ = io_read8(io_base, VIRTIO_REG_ISR_STATUS);
            }
        }
    }

    // Free all 3 descriptors.
    drv.queue.free_desc(desc_hdr);
    drv.queue.free_desc(desc_data);
    drv.queue.free_desc(desc_status);

    // Check the status byte.
    let status_byte = drv.buffers[status_offset];
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
        data.copy_from_slice(&drv.buffers[data_offset..data_offset + SECTOR_SIZE]);
    }

    Ok(())
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

    let Some(dev) = pci::find_device(pci::VIRTIO_VENDOR_ID, pci::VIRTIO_BLK_DEVICE_ID, None) else {
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
        // I/O port BAR -- mask off the type bits.
        (dev.bar0 & 0xFFFC) as u16
    } else {
        // MMIO BAR -- not supported in this driver. We require I/O port.
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

    // Steps 1-3: Reset, ACKNOWLEDGE, DRIVER.
    // SAFETY: `io_base` is a valid virtio I/O port base from PCI BAR0.
    unsafe {
        virtio::init_device(io_base);
    }

    // Step 4: Feature negotiation.
    // We negotiate minimal features for a basic block device.
    let requested = VIRTIO_BLK_F_SIZE_MAX | VIRTIO_BLK_F_BLK_SIZE;
    // SAFETY: `io_base` is a valid virtio I/O port base.
    let guest_features = unsafe { virtio::negotiate_features(io_base, requested) };

    crate::serial_println!("[BLK] Features: negotiated={:#x}", guest_features);

    // Step 5: Features OK.
    // SAFETY: `io_base` is a valid virtio I/O port base.
    if !unsafe { virtio::set_features_ok(io_base) } {
        crate::serial_println!("[BLK] ERROR: device rejected feature negotiation");
        return false;
    }

    // Step 6: Read disk capacity from device config space.
    // In legacy mode, the capacity (in 512-byte sectors) is at I/O base + 0x14.
    let sector_count = unsafe { io_read64(io_base, BLK_CONFIG_OFFSET) };
    let size_mb = (sector_count * SECTOR_SIZE as u64) / (1024 * 1024);
    crate::serial_println!("[BLK] Capacity: {} sectors ({} MiB)", sector_count, size_mb);

    // Step 7: Set up virtqueue (queue 0 -- the only queue for block devices).
    let queue = VirtQueue::new();

    // Allocate a single 4 KiB buffer frame for request headers, data, and status.
    let buf_phys = crate::frame_alloc::alloc_frame().expect("out of frames for virtio-blk buffers");
    let buf_virt = crate::memory::phys_to_virt(buf_phys);
    // SAFETY: Frame is exclusively allocated, zeroed.
    let buffers = unsafe {
        core::ptr::write_bytes(buf_virt as *mut u8, 0, 4096);
        core::slice::from_raw_parts_mut(buf_virt as *mut u8, 4096)
    };

    crate::serial_println!("[BLK] Setting up queue (0)...");
    // SAFETY: I/O base is valid -- we're in the init path after PCI probe.
    unsafe {
        queue.enable_on_device(io_base, 0);
    }

    // Step 8: Set DRIVER_OK -- the device is now live.
    // SAFETY: `io_base` is a valid virtio I/O port base.
    unsafe {
        virtio::set_driver_ok(io_base);
    }

    let driver = VirtioBlockDriver {
        pci: dev,
        io_base,
        sector_count,
        queue,
        buffers,
        buf_phys,
    };

    *DRIVER.lock() = Some(driver);
    crate::serial_println!("[OK] virtio-blk initialized (legacy I/O port)");
    true
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

#[cfg(test)]
mod tests {
    use super::*;

    // ─────────────────── Feature bit tests ───────────────────

    #[test]
    fn virtio_blk_f_size_max() {
        assert_eq!(VIRTIO_BLK_F_SIZE_MAX, 1 << 1);
    }

    #[test]
    fn virtio_blk_f_seg_max() {
        assert_eq!(VIRTIO_BLK_F_SEG_MAX, 1 << 2);
    }

    #[test]
    fn virtio_blk_f_geometry() {
        assert_eq!(VIRTIO_BLK_F_GEOMETRY, 1 << 4);
    }

    #[test]
    fn virtio_blk_f_ro() {
        // Read-only feature is bit 5.
        assert_eq!(VIRTIO_BLK_F_RO, 1 << 5);
    }

    #[test]
    fn virtio_blk_f_blk_size() {
        assert_eq!(VIRTIO_BLK_F_BLK_SIZE, 1 << 6);
    }

    // ─────────────────── Request type tests ───────────────────

    #[test]
    fn virtio_blk_t_in() {
        assert_eq!(VIRTIO_BLK_T_IN, 0);
    }

    #[test]
    fn virtio_blk_t_out() {
        assert_eq!(VIRTIO_BLK_T_OUT, 1);
    }

    #[test]
    fn virtio_blk_s_ok() {
        assert_eq!(VIRTIO_BLK_S_OK, 0);
    }

    #[test]
    fn request_types_are_distinct() {
        assert_ne!(VIRTIO_BLK_T_IN, VIRTIO_BLK_T_OUT);
    }

    // ─────────────────── Geometry tests ───────────────────

    #[test]
    fn sector_size_is_512() {
        assert_eq!(SECTOR_SIZE, 512);
    }

    // ─────────────────── Header layout tests ───────────────────

    #[test]
    fn blk_req_header_sizeof() {
        // #[repr(C)]: u32 + u32 + u64 = 4 + 4 + 8 = 16 bytes.
        assert_eq!(core::mem::size_of::<VirtioBlkReqHeader>(), 16);
    }

    #[test]
    fn blk_req_header_fields() {
        let hdr = VirtioBlkReqHeader {
            request_type: VIRTIO_BLK_T_IN,
            reserved: 0,
            sector: 42,
        };
        assert_eq!(hdr.request_type, 0);
        assert_eq!(hdr.reserved, 0);
        assert_eq!(hdr.sector, 42);
    }

    #[test]
    fn blk_config_offset() {
        assert_eq!(BLK_CONFIG_OFFSET, 0x14);
    }

    // ─────────────────── Public API defaults ───────────────────

    #[test]
    fn sector_count_returns_zero_when_not_initialized() {
        assert_eq!(sector_count(), 0);
    }

    #[test]
    fn read_sector_fails_when_not_initialized() {
        let mut buf = [0u8; SECTOR_SIZE];
        assert!(read_sector(0, &mut buf).is_err());
    }

    #[test]
    fn write_sector_fails_when_not_initialized() {
        let buf = [0u8; SECTOR_SIZE];
        assert!(write_sector(0, &buf).is_err());
    }
}
