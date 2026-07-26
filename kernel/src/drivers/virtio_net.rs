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
//! ## Legacy `VirtIO` I/O Port Interface
//!
//! This driver uses the legacy (transitional) virtio I/O port interface
//! because QEMU's `virtio-net-pci` defaults to legacy mode when the
//! transport is PCI. The virtqueues use the "split virtqueue" layout:
//!
//! - Descriptor Table: array of `(addr, len, flags, next)` entries
//! - Available Ring: driver → device (which descriptors are ready)
//! - Used Ring: device → driver (which descriptors the device consumed)
//!
//! Queue numbering:
//! - Queue 0: Receive (device → driver)
//! - Queue 1: Transmit (driver → device)
//!
//! ## Buffer Management
//!
//! Each descriptor slot has a pre-allocated heap buffer. The physical
//! address of each buffer is stored alongside so it can be placed in the
//! descriptor's `addr` field (the device reads from physical memory).
//! A free-list chain threaded through `next` allows descriptor reuse.

use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{fence, Ordering};

use spin::Mutex;

use super::pci::{self, PciDevice};

// ---------------------------------------------------------------------------
// VirtIO-Net feature bits
// ---------------------------------------------------------------------------

/// Device provides a MAC address in the config space.
const VIRTIO_NET_F_MAC: u64 = 1 << 5;

// ---------------------------------------------------------------------------
// Virtqueue geometry
// ---------------------------------------------------------------------------

/// Number of descriptors per virtqueue (must be a power of two).
const VQ_SIZE: usize = 16;

/// Maximum Ethernet frame payload (excluding virtio-net header).
const MAX_FRAME_SIZE: usize = 1518;

/// Size of the virtio-net header prepended to every packet.
const VIRTIO_NET_HDR_SIZE: usize = core::mem::size_of::<VirtioNetHdr>();

/// Size of one buffer: header + maximum frame.
const BUF_SIZE: usize = VIRTIO_NET_HDR_SIZE + MAX_FRAME_SIZE;

/// Page size used by legacy virtio for queue alignment.
const PAGE_SIZE: u64 = 4096;

// ---------------------------------------------------------------------------
// Legacy virtio I/O port register offsets
//
// From the VirtIO 1.0 spec, Appendix B (Legacy Interface):
// https://docs.oasis-open.org/virtio/virtio/v1.1/csprd01/virtio-v1.1-csprd01.html#x1-1060002
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
// VirtIO-Net header (prepended to every packet)
//
// This is the "legacy" net header without VIRTIO_NET_F_MRG_RXBUF.
// QEMU sends 10 bytes (no num_buffers) unless MRG_RXBUF is negotiated.
// We keep the 12-byte struct for alignment but only the first 10 bytes
// are used on the wire when MRG_RXBUF is not negotiated.
// ---------------------------------------------------------------------------

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
    descriptors: Vec<VirtqDesc>,
    /// Available ring — driver writes descriptor indices here.
    avail: VirtqAvail,
    /// Used ring — device writes completed descriptor indices here.
    used: VirtqUsed,

    /// Physical address of the descriptor table.
    desc_phys: u64,
    /// Physical address of the available ring.
    avail_phys: u64,
    /// Physical address of the used ring.
    used_phys: u64,

    /// Heap-allocated buffers. Each buffer's physical address is stored
    /// in `buf_phys[i]` so it can be written into the descriptor.
    buffers: Vec<Vec<u8>>,
    /// Physical addresses of each buffer (parallel to `buffers`).
    buf_phys: Vec<u64>,

    /// Free descriptor list — indices of descriptors not currently in use.
    /// Threaded through the descriptor's `next` field.
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

/// VirtIO-Net driver state.
struct VirtioNetDriver {
    /// PCI device info — kept for debugging and potential reset.
    #[allow(dead_code)]
    pci: PciDevice,
    /// I/O port base address from BAR0.
    io_base: u16,
    /// MAC address read from device config space.
    mac: [u8; 6],
    /// RX virtqueue (queue 0).
    rx_queue: VirtQueue,
    /// TX virtqueue (queue 1).
    tx_queue: VirtQueue,
}

/// Global driver state, protected by a spin lock.
static DRIVER: Mutex<Option<VirtioNetDriver>> = Mutex::new(None);

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
#[allow(dead_code)]
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

// ---------------------------------------------------------------------------
// Physical address helpers
// ---------------------------------------------------------------------------

/// Convert a virtual address to a physical address.
///
/// Uses the bootloader's `physical_memory_offset` which maps
/// `physical → virtual` as `virtual = physical + offset`.
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
    virt.wrapping_sub(offset)
}

// ---------------------------------------------------------------------------
// VirtQueue implementation
// ---------------------------------------------------------------------------

impl VirtQueue {
    /// Create and initialize a new virtqueue.
    ///
    /// Allocates the descriptor table, available ring, and used ring in
    /// heap memory. Computes their physical addresses so the device can
    /// DMA into them. Initializes the free descriptor list.
    fn new() -> Self {
        let mut descriptors = Vec::with_capacity(VQ_SIZE);
        let mut buffers = Vec::with_capacity(VQ_SIZE);
        let mut buf_phys = Vec::with_capacity(VQ_SIZE);

        for _ in 0..VQ_SIZE {
            descriptors.push(VirtqDesc {
                addr: 0,
                len: 0,
                flags: 0,
                // Thread free list: descriptor i points to i+1.
                next: 0,
            });
            let buf = vec![0u8; BUF_SIZE];
            let phys = virt_to_phys(buf.as_ptr() as u64);
            buf_phys.push(phys);
            buffers.push(buf);
        }

        // Set up free descriptor chain: 0→1→2→...→(VQ_SIZE-1)→END.
        for (i, desc) in descriptors.iter_mut().enumerate().take(VQ_SIZE - 1) {
            desc.next = (i + 1) as u16;
        }
        descriptors[VQ_SIZE - 1].next = 0xFFFF; // end-of-chain sentinel

        let avail = VirtqAvail {
            flags: 0,
            idx: 0,
            ring: [0; VQ_SIZE],
        };
        let used = VirtqUsed {
            flags: 0,
            idx: 0,
            ring: [VirtqUsedElem { id: 0, len: 0 }; VQ_SIZE],
        };

        let desc_phys = virt_to_phys(descriptors.as_ptr() as u64);
        let avail_phys = virt_to_phys(core::ptr::addr_of!(avail) as u64);
        let used_phys = virt_to_phys(core::ptr::addr_of!(used) as u64);

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
    /// # Layout (per the spec, Section 2.6.2 legacy)
    ///
    /// The legacy virtqueue is a single contiguous region:
    ///   `[Descriptor Table] [Available Ring] [Padding] [Used Ring]`
    ///
    /// However, QEMU's legacy implementation accepts the descriptor
    /// table PFN and infers the rest from queue size. We write the
    /// descriptor table's PFN as the queue address.
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
            "[NET]   Queue {}: device reports size {}",
            queue_index,
            device_queue_size
        );
        // The device may support a different size; we use the minimum.
        // For simplicity, we assume VQ_SIZE <= device_queue_size.

        // 3. Write the page frame number of the descriptor table.
        // Legacy virtio expects the PFN (physical address >> 12).
        let pfn = (self.desc_phys / PAGE_SIZE) as u32;
        // SAFETY: Writing queue PFN to valid legacy virtio register.
        unsafe {
            io_write32(io_base, VIRTIO_REG_QUEUE_PFN, pfn);
        }

        crate::serial_println!(
            "[NET]   Queue {}: PFN={:#x} (desc_phys={:#x})",
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
    /// This is the final step of both TX and RX paths: the descriptor
    /// has been filled in, now we tell the device about it.
    ///
    /// # Safety
    /// `io_base` must be a valid virtio I/O port base. The queue index
    /// is written to the notify register so the device knows which
    /// queue to check.
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
// VirtIO-Net driver initialization
// ---------------------------------------------------------------------------

/// Initialize the virtio-net driver.
///
/// Scans the PCI bus for a virtio-net device, negotiates features,
/// sets up the RX and TX virtqueues, pre-submits RX buffers, and
/// marks the device as `DRIVER_OK`.
#[allow(clippy::too_many_lines)]
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
        // I/O port BAR — mask off the type bits.
        (dev.bar0 & 0xFFFC) as u16
    } else {
        // MMIO BAR — not supported in this driver. We require I/O port.
        crate::serial_println!("[NET] ERROR: MMIO BAR not supported, need I/O port BAR");
        return;
    };

    crate::serial_println!("[NET] I/O base: {:#x}", io_base);

    // ---------------------------------------------------------------
    // Legacy virtio initialization sequence (spec Section 3.1.1):
    //   1. Reset device (status = 0)
    //   2. Set ACKNOWLEDGE
    //   3. Set DRIVER
    //   4. Read and negotiate feature bits
    //   5. Set FEATURES_OK
    //   6. Set up virtqueues
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
    // We only need the MAC feature bit.
    let guest_features = device_features as u64 & VIRTIO_NET_F_MAC;
    // SAFETY: Writing to valid virtio feature register.
    unsafe {
        io_write32(io_base, VIRTIO_REG_GUEST_FEATURES, guest_features as u32);
    }

    crate::serial_println!(
        "[NET] Features: device={:#x}, negotiated={:#x}",
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
        crate::serial_println!("[NET] ERROR: device rejected feature negotiation");
        // Set FAILED status.
        // SAFETY: Writing to valid virtio device status register.
        unsafe {
            io_write8(io_base, VIRTIO_REG_DEVICE_STATUS, STATUS_FAILED);
        }
        return;
    }

    // Step 6: Read MAC address from device config space.
    // For legacy virtio-net, the MAC is at I/O base + 0x14.
    let mut mac = [0u8; 6];
    // SAFETY: Port I/O from valid virtio-net device config space.
    // Legacy virtio-net config starts at offset 0x14 from I/O base.
    let mac_lo: u32 = unsafe {
        let mut port = x86_64::instructions::port::Port::new(io_base + 0x14);
        port.read()
    };
    // SAFETY: Port I/O from valid virtio-net device config space.
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

    // Step 7: Set up virtqueues.
    // Queue 0 = RX, Queue 1 = TX.
    let mut rx_queue = VirtQueue::new();
    let tx_queue = VirtQueue::new();

    crate::serial_println!("[NET] Setting up RX queue (0)...");
    // SAFETY: I/O base is valid — we're in the init path after PCI probe.
    unsafe {
        rx_queue.enable_on_device(io_base, 0);
    }
    crate::serial_println!("[NET] Setting up TX queue (1)...");
    // SAFETY: I/O base is valid — we're in the init path after PCI probe.
    unsafe {
        tx_queue.enable_on_device(io_base, 1);
    }

    // Pre-submit all RX buffers so the device can fill them.
    crate::serial_println!("[NET] Pre-submitting {} RX buffers...", VQ_SIZE);
    let hdr_size = VIRTIO_NET_HDR_SIZE;
    for i in 0..VQ_SIZE {
        let desc_idx = rx_queue.alloc_desc().unwrap_or_else(|| {
            crate::serial_println!("[NET] WARNING: ran out of RX descriptors at {}", i);
            // This should not happen since we have exactly VQ_SIZE descriptors.
            u16::MAX
        });
        if desc_idx == u16::MAX {
            break;
        }
        // Set up the descriptor to point at this buffer.
        // The buffer is device-writable (DESC_F_WRITE) because the device
        // will write received frame data into it.
        rx_queue.descriptors[desc_idx as usize].addr = rx_queue.buf_phys[desc_idx as usize];
        rx_queue.descriptors[desc_idx as usize].len = BUF_SIZE as u32;
        rx_queue.descriptors[desc_idx as usize].flags = DESC_F_WRITE;

        // Submit to the available ring.
        // SAFETY: I/O base is valid, queue index 0 = RX.
        unsafe {
            rx_queue.submit_and_notify(desc_idx, io_base, 0);
        }
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

    let driver = VirtioNetDriver {
        pci: dev,
        io_base,
        mac,
        rx_queue,
        tx_queue,
    };

    *DRIVER.lock() = Some(driver);
    crate::serial_println!("[OK] virtio-net initialized (legacy I/O port)");
}

// ---------------------------------------------------------------------------
// TX path
// ---------------------------------------------------------------------------

/// Send a raw Ethernet frame.
///
/// Copies the frame into a pre-allocated buffer, sets up a descriptor
/// pointing at it (with an all-zero virtio-net header), submits it to
/// the TX virtqueue's available ring, and notifies the device.
///
/// # Errors
/// Returns an error if the driver is not initialized, the frame is too
/// large, or the TX queue is full (all descriptors in flight).
pub fn send_frame(data: &[u8]) -> Result<usize, &'static str> {
    let mut driver = DRIVER.lock();
    let drv = driver.as_mut().ok_or("not initialized")?;

    if data.len() > MAX_FRAME_SIZE {
        return Err("frame too large");
    }

    let desc_idx = drv.tx_queue.alloc_desc().ok_or("tx queue full")?;
    let idx = desc_idx as usize;
    let buf = &mut drv.tx_queue.buffers[idx];

    // Write an all-zero virtio-net header (no offload features negotiated).
    buf[..VIRTIO_NET_HDR_SIZE].fill(0);

    // Copy frame data after the header.
    let copy_len = data.len().min(MAX_FRAME_SIZE);
    buf[VIRTIO_NET_HDR_SIZE..VIRTIO_NET_HDR_SIZE + copy_len].copy_from_slice(&data[..copy_len]);

    // Set up the descriptor. The buffer is device-readable (no DESC_F_WRITE)
    // because the device reads the frame from this buffer to transmit it.
    let total_len = (VIRTIO_NET_HDR_SIZE + copy_len) as u32;
    drv.tx_queue.descriptors[idx].addr = drv.tx_queue.buf_phys[idx];
    drv.tx_queue.descriptors[idx].len = total_len;
    drv.tx_queue.descriptors[idx].flags = 0;

    // Submit to available ring and notify device.
    // SAFETY: `io_base` is valid for this initialized driver.
    unsafe {
        drv.tx_queue.submit_and_notify(desc_idx, drv.io_base, 1);
    }

    crate::serial_println!("[NET] TX: {} bytes (desc {})", copy_len, desc_idx);
    Ok(copy_len)
}

// ---------------------------------------------------------------------------
// RX path
// ---------------------------------------------------------------------------

/// Receive a raw Ethernet frame (non-blocking).
///
/// Checks the RX virtqueue's used ring for completed buffers. If the
/// device has filled a buffer with a received frame, returns the frame
/// data (without the virtio-net header). The descriptor is re-submitted
/// so the device can fill it again.
///
/// Returns `None` if no frame is available.
pub fn receive_frame() -> Option<Vec<u8>> {
    let mut driver = DRIVER.lock();
    let drv = driver.as_mut()?;

    // Poll the used ring for completed RX descriptors.
    let (desc_idx, len) = drv.rx_queue.poll_used()?;
    let idx = desc_idx as usize;

    // The device wrote `len` bytes total (header + frame).
    if len as usize <= VIRTIO_NET_HDR_SIZE {
        // Header-only or zero-length — nothing useful. Re-submit.
        // SAFETY: `io_base` is valid.
        unsafe {
            drv.rx_queue.submit_and_notify(desc_idx, drv.io_base, 0);
        }
        return None;
    }

    let frame_len = len as usize - VIRTIO_NET_HDR_SIZE;
    let buf = &drv.rx_queue.buffers[idx];
    let frame_data = buf[VIRTIO_NET_HDR_SIZE..VIRTIO_NET_HDR_SIZE + frame_len].to_vec();

    // Re-submit the descriptor so the device can receive the next frame.
    // Reset the descriptor fields (addr/len/flags are still correct, but
    // clear the length to the full buffer size in case the device wrote less).
    drv.rx_queue.descriptors[idx].len = BUF_SIZE as u32;
    drv.rx_queue.descriptors[idx].flags = DESC_F_WRITE;
    // SAFETY: `io_base` is valid.
    unsafe {
        drv.rx_queue.submit_and_notify(desc_idx, drv.io_base, 0);
    }

    crate::serial_println!("[NET] RX: {} bytes (desc {})", frame_len, desc_idx);
    Some(frame_data)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Get the MAC address of the virtio-net device.
///
/// Returns `[0; 6]` if the driver is not initialized.
pub fn mac_address() -> [u8; 6] {
    DRIVER.lock().as_ref().map_or([0; 6], |d| d.mac)
}
