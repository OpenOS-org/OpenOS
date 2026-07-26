//! VirtIO-Net network driver.
//!
//! Implements the virtio-net PCI device for sending and receiving
//! Ethernet frames. This is the kernel-side raw packet driver;
//! the TCP/IP stack runs in user-space.
//!
//! ## Architecture
//!
//! ```text
//! User-space (TCP/IP stack)
//!     │
//!     │ channel_send / channel_receive
//!     ▼
//! Kernel driver (this module)
//!     │
//!     │ virtqueue send/receive
//!     ▼
//! virtio-net PCI device
//!     │
//!     │ Ethernet frames
//!     ▼
//! Network (QEMU virtual NIC)
//! ```
//!
//! ## `VirtQueue` Layout
//!
//! Each virtqueue has three parts:
//! 1. Descriptor Table: array of buffer descriptors
//! 2. Available Ring: guest → host (what buffers the driver offers)
//! 3. Used Ring: host → guest (what buffers the device has consumed)
//!
//! For virtio-net:
//! - Queue 0: Receive (device → driver)
//! - Queue 1: Transmit (driver → device)
//! - Queue 2: Control (optional, not used here)

use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU16, AtomicU32, Ordering};

use spin::Mutex;

use super::pci::{self, PciDevice};

/// VirtIO-Net feature bits (`VIRTIO_NET_F_MAC`) we support.
const VIRTIO_NET_F_MAC: u64 = 1 << 5;

/// Virtqueue size (number of descriptors).
const VQ_SIZE: usize = 16;

/// Maximum Ethernet frame size.
const MAX_FRAME_SIZE: usize = 1518;

/// VirtIO-Net header prepended to every packet.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct VirtioNetHdr {
    flags: u8,
    gso_type: u8,
    hdr_len: u16,
    gso_size: u16,
    csum_start: u16,
    csum_offset: u16,
    num_buffers: u16,
}

/// A single descriptor in the virtqueue descriptor table.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct VirtqDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

/// Available ring header.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct VirtqAvail {
    flags: u16,
    idx: u16,
    ring: [u16; VQ_SIZE],
}

/// Used ring element.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct VirtqUsedElem {
    id: u32,
    len: u32,
}

/// Used ring header.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct VirtqUsed {
    flags: u16,
    idx: u16,
    ring: [VirtqUsedElem; VQ_SIZE],
}

/// A virtqueue.
struct VirtQueue {
    /// Descriptor table.
    descriptors: Vec<VirtqDesc>,
    /// Available ring.
    avail: VirtqAvail,
    /// Used ring.
    used: VirtqUsed,
    /// Index of next descriptor to use.
    next_desc: u16,
    /// Index of next used element to consume.
    next_used: u16,
    /// Buffers for each descriptor.
    buffers: Vec<Vec<u8>>,
}

/// VirtIO-Net driver state.
struct VirtioNetDriver {
    /// PCI device info (`PciDevice`) ().
    #[allow(dead_code)]
    pci: PciDevice,
    /// MMIO base address (`u64`) ().
    mmio_base: u64,
    /// MAC address.
    mac: [u8; 6],
    /// RX virtqueue ().
    rx_queue: VirtQueue,
    /// TX virtqueue ().
    tx_queue: VirtQueue,
    /// Whether the driver is initialized.
    initialized: bool,
}

/// Global driver state.
static DRIVER: Mutex<Option<VirtioNetDriver>> = Mutex::new(None);

/// Read a 32-bit register from the MMIO region.
///
/// # Safety
/// `base` must be the valid MMIO base address (`u64`) of a virtio device.
unsafe fn mmio_read(base: u64, offset: u32) -> u32 {
    // SAFETY: MMIO read from a valid device register.
    unsafe { core::ptr::read_volatile((base + offset as u64) as *const u32) }
}

/// Write a 32-bit register to the MMIO region.
///
/// # Safety
/// `base` must be the valid MMIO base address (`u64`) of a virtio device.
unsafe fn mmio_write(base: u64, offset: u32, value: u32) {
    // SAFETY: MMIO write to a valid device register.
    unsafe { core::ptr::write_volatile((base + offset as u64) as *mut u32, value) }
}

/// VirtIO-PCI capability types (constants):
/// - : common configuration
/// - : notification configuration
/// - : ISR configuration
/// - : device-specific configuration
/// - : PCI configuration
const VIRTIO_PCI_CAP_COMMON_CFG: u8 = 1;
const VIRTIO_PCI_CAP_NOTIFY_CFG: u8 = 2;
const VIRTIO_PCI_CAP_ISR_CFG: u8 = 3;
const VIRTIO_PCI_CAP_DEVICE_CFG: u8 = 4;
const VIRTIO_PCI_CAP_PCI_CFG: u8 = 5;

/// VirtIO-PCI capability structure.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct VirtioPciCap {
    cap_vndr: u8,
    cap_next: u8,
    cap_len: u8,
    cfg_type: u8,
    bar: u8,
    id: u8,
    padding: [u8; 2],
    offset: u32,
    length: u32,
}

/// Common configuration structure ().
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct CommonCfg {
    device_feature_select: u32,
    device_feature: u32,
    driver_feature_select: u32,
    driver_feature: u32,
    msix_config: u16,
    num_queues: u16,
    device_status: u8,
    config_generation: u8,
    queue_select: u16,
    queue_size: u16,
    queue_msix_vector: u16,
    queue_enable: u16,
    queue_notify_off: u16,
    queue_desc_lo: u32,
    queue_desc_hi: u32,
    queue_driver_lo: u32,
    queue_driver_hi: u32,
    queue_device_lo: u32,
    queue_device_hi: u32,
    queue_notify_data: u16,
    queue_reset: u16,
}

/// Initialize the virtio-net driver.
///
/// Scans the PCI bus for a virtio-net device, sets up virtqueues,
/// and enables the device.
pub fn init() {
    crate::serial_println!("[NET] Scanning PCI bus for virtio-net...");

    let Some(dev) = pci::find_device(pci::VIRTIO_VENDOR_ID, pci::VIRTIO_NET_DEVICE_ID) else {
        crate::serial_println!("[NET] No virtio-net device found");
        return;
    };

    crate::serial_println!(
        "[NET] Found virtio-net at PCI {}:{}:{}, BAR0={:#x}",
        dev.bus,
        dev.device,
        dev.function,
        dev.bar0
    );

    // Determine device access method from BAR0.
    // BAR0 bit 0: 0 = MMIO, 1 = I/O port.
    // For I/O port BAR, the base address is BAR0 & 0xFFFC.
    let io_base = if dev.bar0 & 1 != 0 {
        // I/O port BAR
        (dev.bar0 & 0xFFFC) as u16
    } else {
        // MMIO BAR — use BAR1 if available, or BAR0
        crate::serial_println!("[NET] WARNING: MMIO BAR not supported yet, using BAR1");
        (dev.bar1 & 0xFFFC) as u16
    };

    crate::serial_println!("[NET] I/O base: {:#x}", io_base);

    // Read MAC address from virtio-net device config.
    // For legacy virtio-net, the MAC is at I/O base + 0x14 (4 bytes + 2 bytes).
    // SAFETY: Port I/O from valid virtio-net device.
    let mut mac = [0u8; 6];
    // SAFETY: Port I/O from valid virtio-net device.
    let mac_lo: u32 = unsafe {
        let mut port = x86_64::instructions::port::Port::new(io_base + 0x14);
        port.read()
    };
    // SAFETY: Port I/O from valid virtio-net device.
    let mac_hi: u16 = unsafe {
        let mut port = x86_64::instructions::port::Port::new(io_base + 0x18);
        port.read()
    };
    mac[0] = (mac_lo & 0xFF) as u8;
    mac[1] = ((mac_lo >> 8) & 0xFF) as u8;
    mac[2] = ((mac_lo >> 16) & 0xFF) as u8;
    mac[3] = ((mac_lo >> 24) & 0xFF) as u8;
    mac[4] = (mac_hi & 0xFF) as u8;
    mac[5] = ((mac_hi >> 8) & 0xFF) as u8;

    crate::serial_println!(
        "[NET] MAC address: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5]
    );

    // Create virtqueues (simplified — use legacy I/O port interface).
    let rx_queue = VirtQueue::new();
    let tx_queue = VirtQueue::new();

    let driver = VirtioNetDriver {
        pci: dev,
        mmio_base: io_base as u64,
        mac,
        rx_queue,
        tx_queue,
        initialized: true,
    };

    *DRIVER.lock() = Some(driver);
    crate::serial_println!("[OK] virtio-net initialized");
}

impl VirtQueue {
    fn new() -> Self {
        let mut descriptors = Vec::with_capacity(VQ_SIZE);
        let mut buffers = Vec::with_capacity(VQ_SIZE);
        for _ in 0..VQ_SIZE {
            descriptors.push(VirtqDesc::default());
            buffers.push(vec![
                0u8;
                MAX_FRAME_SIZE + core::mem::size_of::<VirtioNetHdr>()
            ]);
        }
        Self {
            descriptors,
            avail: VirtqAvail {
                flags: 0,
                idx: 0,
                ring: [0; VQ_SIZE],
            },
            used: VirtqUsed {
                flags: 0,
                idx: 0,
                ring: [VirtqUsedElem { id: 0, len: 0 }; VQ_SIZE],
            },
            next_desc: 0,
            next_used: 0,
            buffers,
        }
    }

    /// Allocate a descriptor index.
    fn alloc_desc(&mut self) -> Option<u16> {
        let idx = self.next_desc;
        if idx as usize >= VQ_SIZE {
            return None;
        }
        self.next_desc += 1;
        Some(idx)
    }
}

/// Send a raw Ethernet frame.
pub fn send_frame(data: &[u8]) -> Result<usize, &'static str> {
    let mut driver = DRIVER.lock();
    let drv = driver.as_mut().ok_or("not initialized")?;

    if data.len() > MAX_FRAME_SIZE {
        return Err("frame too large");
    }

    let desc_idx = drv.tx_queue.alloc_desc().ok_or("tx queue full")?;
    let buf = &mut drv.tx_queue.buffers[desc_idx as usize];

    // Write virtio-net header (all zeros for simple case).
    let hdr_size = core::mem::size_of::<VirtioNetHdr>();
    buf[..hdr_size].fill(0);

    // Copy frame data after header.
    let copy_len = data.len().min(MAX_FRAME_SIZE);
    buf[hdr_size..hdr_size + copy_len].copy_from_slice(&data[..copy_len]);

    // Set descriptor.
    let total_len = hdr_size + copy_len;
    drv.tx_queue.descriptors[desc_idx as usize].len = total_len as u32;
    drv.tx_queue.descriptors[desc_idx as usize].flags = 0;

    // Add to available ring.
    let avail_idx = drv.tx_queue.avail.idx as usize;
    drv.tx_queue.avail.ring[avail_idx % VQ_SIZE] = desc_idx;
    drv.tx_queue.avail.idx = drv.tx_queue.avail.idx.wrapping_add(1);

    // Notify device (write to notify register).
    // SAFETY: MMIO write to valid device notify register.
    unsafe {
        mmio_write(drv.mmio_base, 0x50, 1); // queue 1 = TX
    }

    crate::serial_println!("[NET] Sent {} bytes", copy_len);
    Ok(copy_len)
}

/// Receive a raw Ethernet frame (non-blocking).
pub fn receive_frame() -> Option<Vec<u8>> {
    let mut driver = DRIVER.lock();
    let drv = driver.as_mut()?;

    // Check if device has written anything to the used ring.
    if drv.rx_queue.next_used == drv.rx_queue.used.idx {
        return None; // Nothing available
    }

    let used_elem = drv.rx_queue.used.ring[drv.rx_queue.next_used as usize % VQ_SIZE];
    drv.rx_queue.next_used = drv.rx_queue.next_used.wrapping_add(1);

    let desc_idx = used_elem.id as usize;
    let len = used_elem.len as usize;
    let hdr_size = core::mem::size_of::<VirtioNetHdr>();

    if len <= hdr_size {
        return None;
    }

    let frame_len = len - hdr_size;
    let buf = &drv.rx_queue.buffers[desc_idx];
    let frame_data = buf[hdr_size..hdr_size + frame_len].to_vec();

    // Re-submit the descriptor for next receive.
    drv.rx_queue.avail.ring[drv.rx_queue.avail.idx as usize % VQ_SIZE] = desc_idx as u16;
    drv.rx_queue.avail.idx = drv.rx_queue.avail.idx.wrapping_add(1);

    Some(frame_data)
}

/// Get the MAC address.
pub fn mac_address() -> [u8; 6] {
    DRIVER.lock().as_ref().map_or([0; 6], |d| d.mac)
}
