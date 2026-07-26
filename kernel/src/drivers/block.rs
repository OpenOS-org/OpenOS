//! Block device abstraction.
//!
//! Provides a trait-based interface for block storage devices and a global
//! registry that holds up to 4 devices. The VirtIO-Block driver is wrapped
//! as the first concrete implementation.
//!
//! ## Architecture
//!
//! ```text
//! Filesystem / block_cache
//!     │
//!     │ get_device(index)
//!     ▼
//! BlockDevice trait (dyn dispatch)
//!     │
//!     │ read_sector / write_sector
//!     ▼
//! VirtIO-Block (or future drivers)
//! ```

use alloc::sync::Arc;

use spin::Mutex;

/// Sector size in bytes, used by all block devices.
const SECTOR_SIZE: usize = 512;

/// Maximum number of registered block devices.
const MAX_DEVICES: usize = 4;

/// Abstract block device interface.
///
/// Every block storage backend implements this trait. The methods operate
/// on fixed-size 512-byte sectors addressed by linear block address (LBA).
pub trait BlockDevice: Send + Sync {
    /// Read a single sector from the device into `buf`.
    ///
    /// # Errors
    /// Returns `Err(())` if the read fails (device error, out of range, etc.).
    fn read_sector(&self, lba: u64, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), ()>;

    /// Write a single sector from `buf` to the device.
    ///
    /// # Errors
    /// Returns `Err(())` if the write fails (device error, read-only, etc.).
    fn write_sector(&self, lba: u64, buf: &[u8; SECTOR_SIZE]) -> Result<(), ()>;

    /// Return the sector size in bytes. Default is 512.
    fn sector_size(&self) -> usize {
        SECTOR_SIZE
    }

    /// Return the total number of sectors on this device.
    fn sector_count(&self) -> u64;
}

/// Global block device registry.
///
/// Holds up to `MAX_DEVICES` device slots. Devices are registered at boot
/// and looked up by index for the duration of the kernel's lifetime.
static BLOCK_DEVICES: Mutex<[Option<Arc<dyn BlockDevice>>; MAX_DEVICES]> =
    Mutex::new([None, None, None, None]);

/// Register a block device in the global registry.
///
/// Finds the first empty slot and stores the device. Returns without
/// action if all slots are full.
pub fn register_device(dev: Arc<dyn BlockDevice>) {
    let mut devs = BLOCK_DEVICES.lock();
    for slot in devs.iter_mut() {
        if slot.is_none() {
            *slot = Some(dev);
            return;
        }
    }
    crate::serial_println!("[BLOCK] WARNING: all device slots full, registration failed");
}

/// Retrieve a registered block device by index.
///
/// Returns `None` if the slot is empty or the index is out of range.
pub fn get_device(index: usize) -> Option<Arc<dyn BlockDevice>> {
    if index >= MAX_DEVICES {
        return None;
    }
    BLOCK_DEVICES.lock()[index].clone()
}

// ---------------------------------------------------------------------------
// VirtIO-Block adapter
// ---------------------------------------------------------------------------

/// Adapter that implements [`BlockDevice`] by delegating to the VirtIO-Block
/// driver's global `read_sector` / `write_sector` functions.
///
/// This is a zero-sized type; the VirtIO-Block driver maintains its own
/// global state behind a `Mutex`.
pub struct VirtioBlockAdapter;

impl BlockDevice for VirtioBlockAdapter {
    fn read_sector(&self, lba: u64, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), ()> {
        super::virtio_block::read_sector(lba, buf)
    }

    fn write_sector(&self, lba: u64, buf: &[u8; SECTOR_SIZE]) -> Result<(), ()> {
        super::virtio_block::write_sector(lba, buf)
    }

    fn sector_count(&self) -> u64 {
        super::virtio_block::sector_count()
    }
}
