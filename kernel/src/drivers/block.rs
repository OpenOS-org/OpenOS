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

#[cfg(test)]
mod tests {
    extern crate alloc;

    use alloc::sync::Arc;
    use alloc::vec;
    use core::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    /// A mock block device for testing the registry and trait interface.
    struct MockBlockDevice {
        /// Tracks the number of read_sector calls.
        read_count: AtomicU64,
        /// Tracks the number of write_sector calls.
        write_count: AtomicU64,
        /// Total sectors on this mock device.
        sectors: u64,
    }

    impl MockBlockDevice {
        fn new(sectors: u64) -> Self {
            Self {
                read_count: AtomicU64::new(0),
                write_count: AtomicU64::new(0),
                sectors,
            }
        }
    }

    impl BlockDevice for MockBlockDevice {
        fn read_sector(&self, _lba: u64, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), ()> {
            self.read_count.fetch_add(1, Ordering::Relaxed);
            buf.fill(0xAB);
            Ok(())
        }

        fn write_sector(&self, _lba: u64, _buf: &[u8; SECTOR_SIZE]) -> Result<(), ()> {
            self.write_count.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        fn sector_count(&self) -> u64 {
            self.sectors
        }
    }

    /// A mock device that always fails.
    struct FailingBlockDevice;

    impl BlockDevice for FailingBlockDevice {
        fn read_sector(&self, _lba: u64, _buf: &mut [u8; SECTOR_SIZE]) -> Result<(), ()> {
            Err(())
        }

        fn write_sector(&self, _lba: u64, _buf: &[u8; SECTOR_SIZE]) -> Result<(), ()> {
            Err(())
        }

        fn sector_count(&self) -> u64 {
            0
        }
    }

    #[test]
    fn sector_size_is_512() {
        assert_eq!(SECTOR_SIZE, 512);
    }

    #[test]
    fn max_devices_is_4() {
        assert_eq!(MAX_DEVICES, 4);
    }

    #[test]
    fn get_device_out_of_bounds_returns_none() {
        // Index beyond MAX_DEVICES should always return None.
        assert!(get_device(MAX_DEVICES).is_none());
        assert!(get_device(MAX_DEVICES + 100).is_none());
    }

    #[test]
    fn register_and_get_device() {
        // NOTE: This test interacts with the global BLOCK_DEVICES static.
        // In the test harness (no_std), the static is shared across tests.
        // We verify the core logic by checking that a registered device
        // can be retrieved and its trait methods work.
        let mock = Arc::new(MockBlockDevice::new(1024));
        register_device(mock.clone());

        // The device should be retrievable. We don't know which slot it
        // landed in (other tests may have registered devices), so iterate.
        let mut found = false;
        for i in 0..MAX_DEVICES {
            if let Some(dev) = get_device(i) {
                // Verify it's functional via the trait.
                let mut buf = [0u8; SECTOR_SIZE];
                if dev.read_sector(0, &mut buf).is_ok() {
                    assert_eq!(buf[0], 0xAB);
                    assert_eq!(dev.sector_count(), 1024);
                    found = true;
                    break;
                }
            }
        }
        assert!(found, "registered device should be retrievable");
    }

    #[test]
    fn mock_device_read_counts() {
        let mock = Arc::new(MockBlockDevice::new(512));
        let mut buf = [0u8; SECTOR_SIZE];

        assert_eq!(mock.read_count.load(Ordering::Relaxed), 0);
        mock.read_sector(0, &mut buf).unwrap();
        assert_eq!(mock.read_count.load(Ordering::Relaxed), 1);
        mock.read_sector(10, &mut buf).unwrap();
        assert_eq!(mock.read_count.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn mock_device_write_counts() {
        let mock = Arc::new(MockBlockDevice::new(512));
        let buf = [0u8; SECTOR_SIZE];

        assert_eq!(mock.write_count.load(Ordering::Relaxed), 0);
        mock.write_sector(0, &buf).unwrap();
        assert_eq!(mock.write_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn failing_device_returns_err() {
        let dev = FailingBlockDevice;
        let mut buf = [0u8; SECTOR_SIZE];
        assert!(dev.read_sector(0, &mut buf).is_err());
        assert!(dev.write_sector(0, &buf).is_err());
        assert_eq!(dev.sector_count(), 0);
    }

    #[test]
    fn sector_size_default_impl() {
        // Verify the default trait method returns SECTOR_SIZE.
        let mock = MockBlockDevice::new(100);
        assert_eq!(mock.sector_size(), SECTOR_SIZE);
    }
}
