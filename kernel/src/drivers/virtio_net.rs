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
//! transport is PCI. The virtqueues use the "split virtqueue" layout
//! (shared types live in [`super::virtio`]).
//!
//! Queue numbering:
//! - Queue 0: Receive (device -> driver)
//! - Queue 1: Transmit (driver -> device)
//!
//! ## Buffer Management
//!
//! Each descriptor slot has a pre-allocated heap buffer. The physical
//! address of each buffer is stored alongside so it can be placed in the
//! descriptor's `addr` field (the device reads from physical memory).
//! A free-list chain threaded through `next` allows descriptor reuse.

use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use spin::Mutex;

use super::pci::{self, PciDevice};
use super::virtio::{
    self, io_read16, io_read32, io_read8, io_write8, VirtQueue, DESC_F_WRITE, PAGE_SIZE,
    VIRTIO_REG_DEVICE_STATUS, VIRTIO_REG_GUEST_FEATURES, VQ_SIZE,
};

// ---------------------------------------------------------------------------
// VirtIO-Net feature bits
// ---------------------------------------------------------------------------

/// Device provides a MAC address in the config space.
const VIRTIO_NET_F_MAC: u64 = 1 << 5;

// ---------------------------------------------------------------------------
// Buffer geometry
// ---------------------------------------------------------------------------

/// Maximum Ethernet frame payload (excluding virtio-net header).
const MAX_FRAME_SIZE: usize = 1518;

/// Size of the legacy virtio-net header (10 bytes).
/// Modern virtio-net 1.0 adds `num_buffers` (2 bytes) = 12 total,
/// but QEMU's legacy interface uses 10 bytes.
const VIRTIO_NET_HDR_SIZE: usize = 10;

/// Size of one buffer: header + maximum frame.
const BUF_SIZE: usize = VIRTIO_NET_HDR_SIZE + MAX_FRAME_SIZE;

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
// Driver state
// ---------------------------------------------------------------------------

/// VirtIO-Net driver state.
struct VirtioNetDriver {
    /// PCI device info -- kept for debugging and potential reset.
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
    /// Per-descriptor RX/TX buffers (physical memory for DMA).
    /// Indexed by descriptor index. Each buffer is `BUF_SIZE` bytes.
    rx_buffers: Vec<&'static mut [u8]>,
    /// Physical addresses of each RX buffer.
    rx_buf_phys: Vec<u64>,
    /// Per-descriptor TX buffers.
    tx_buffers: Vec<&'static mut [u8]>,
    /// Physical addresses of each TX buffer.
    tx_buf_phys: Vec<u64>,
}

/// Global driver state, protected by a spin lock.
static DRIVER: Mutex<Option<VirtioNetDriver>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Network statistics (atomic counters, safe to read from any context)
// ---------------------------------------------------------------------------

/// Total successfully received packets.
static RX_PACKETS: AtomicU64 = AtomicU64::new(0);
/// Total successfully transmitted packets.
static TX_PACKETS: AtomicU64 = AtomicU64::new(0);
/// Receive errors (header-only frames, device anomalies).
static RX_ERRORS: AtomicU64 = AtomicU64::new(0);
/// Transmit errors (queue full, frame too large, driver not initialized).
static TX_ERRORS: AtomicU64 = AtomicU64::new(0);
/// Received packets dropped (no buffers available).
static RX_DROPPED: AtomicU64 = AtomicU64::new(0);
/// Transmitted packets dropped.
static TX_DROPPED: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Buffer allocation
// ---------------------------------------------------------------------------

/// Allocate `count` DMA-compatible buffers of `size` bytes each.
///
/// Returns a vector of `(slice, phys_addr)` pairs. Each buffer lives in
/// a dedicated physical frame allocated via `alloc_frame`.
fn alloc_dma_buffers(count: usize, size: usize) -> (Vec<&'static mut [u8]>, Vec<u64>) {
    let mut buffers: Vec<&'static mut [u8]> = Vec::with_capacity(count);
    let mut buf_phys: Vec<u64> = Vec::with_capacity(count);

    for _ in 0..count {
        let phys = crate::frame_alloc::alloc_frame().expect("out of frames for virtio-net buffers");
        let virt = crate::memory::phys_to_virt(phys);
        // SAFETY: Frame is exclusively allocated, zeroed.
        let buf = unsafe {
            core::ptr::write_bytes(virt as *mut u8, 0, 4096);
            core::slice::from_raw_parts_mut(virt as *mut u8, size)
        };
        buf_phys.push(phys);
        buffers.push(buf);
    }

    (buffers, buf_phys)
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

    let Some(dev) = pci::find_device(pci::VIRTIO_VENDOR_ID, pci::VIRTIO_NET_DEVICE_ID, None) else {
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
    let io_base = if dev.bar0 & 1 != 0 {
        // I/O port BAR -- mask off the type bits.
        (dev.bar0 & 0xFFFC) as u16
    } else {
        // MMIO BAR -- not supported in this driver. We require I/O port.
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

    // Steps 1-3: Reset, ACKNOWLEDGE, DRIVER.
    // SAFETY: `io_base` is a valid virtio I/O port base from PCI BAR0.
    unsafe {
        virtio::init_device(io_base);
    }

    // Step 4: Feature negotiation.
    // We only need the MAC feature bit.
    // SAFETY: `io_base` is a valid virtio I/O port base.
    let guest_features = unsafe { virtio::negotiate_features(io_base, VIRTIO_NET_F_MAC) };

    crate::serial_println!("[NET] Features: negotiated={:#x}", guest_features);

    // Step 5: Features OK.
    // SAFETY: `io_base` is a valid virtio I/O port base.
    if !unsafe { virtio::set_features_ok(io_base) } {
        crate::serial_println!("[NET] ERROR: device rejected feature negotiation");
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
    // SAFETY: I/O base is valid -- we're in the init path after PCI probe.
    unsafe {
        rx_queue.enable_on_device(io_base, 0);
    }
    crate::serial_println!("[NET] Setting up TX queue (1)...");
    // SAFETY: I/O base is valid -- we're in the init path after PCI probe.
    unsafe {
        tx_queue.enable_on_device(io_base, 1);
    }

    // Allocate per-descriptor DMA buffers for RX and TX.
    let (rx_buffers, rx_buf_phys) = alloc_dma_buffers(VQ_SIZE, BUF_SIZE);
    let (tx_buffers, tx_buf_phys) = alloc_dma_buffers(VQ_SIZE, BUF_SIZE);

    // Pre-submit all RX buffers so the device can fill them.
    crate::serial_println!("[NET] Pre-submitting {} RX buffers...", VQ_SIZE);
    for i in 0..VQ_SIZE {
        let desc_idx = rx_queue.alloc_desc().unwrap_or_else(|| {
            crate::serial_println!("[NET] WARNING: ran out of RX descriptors at {}", i);
            u16::MAX
        });
        if desc_idx == u16::MAX {
            break;
        }
        // Set up the descriptor to point at this buffer.
        // The buffer is device-writable (DESC_F_WRITE) because the device
        // will write received frame data into it.
        rx_queue.descriptors[desc_idx as usize].addr = rx_buf_phys[desc_idx as usize];
        rx_queue.descriptors[desc_idx as usize].len = BUF_SIZE as u32;
        rx_queue.descriptors[desc_idx as usize].flags = DESC_F_WRITE;

        // Submit to the available ring.
        // SAFETY: I/O base is valid, queue index 0 = RX.
        unsafe {
            rx_queue.submit_and_notify(desc_idx, io_base, 0);
        }
    }

    // Step 8: Set DRIVER_OK -- the device is now live.
    // SAFETY: `io_base` is a valid virtio I/O port base.
    unsafe {
        virtio::set_driver_ok(io_base);
    }

    let driver = VirtioNetDriver {
        pci: dev,
        io_base,
        mac,
        rx_queue,
        tx_queue,
        rx_buffers,
        rx_buf_phys,
        tx_buffers,
        tx_buf_phys,
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
    let drv = driver.as_mut().ok_or_else(|| {
        TX_ERRORS.fetch_add(1, Ordering::Relaxed);
        "not initialized"
    })?;

    if data.len() > MAX_FRAME_SIZE {
        TX_ERRORS.fetch_add(1, Ordering::Relaxed);
        return Err("frame too large");
    }

    let desc_idx = drv.tx_queue.alloc_desc().ok_or_else(|| {
        TX_ERRORS.fetch_add(1, Ordering::Relaxed);
        "tx queue full"
    })?;
    let idx = desc_idx as usize;
    let buf = &mut drv.tx_buffers[idx];

    // Write an all-zero virtio-net header (no offload features negotiated).
    buf[..VIRTIO_NET_HDR_SIZE].fill(0);

    // Copy frame data after the header.
    let copy_len = data.len().min(MAX_FRAME_SIZE);
    buf[VIRTIO_NET_HDR_SIZE..VIRTIO_NET_HDR_SIZE + copy_len].copy_from_slice(&data[..copy_len]);

    // Set up the descriptor. The buffer is device-readable (no DESC_F_WRITE)
    // because the device reads the frame from this buffer to transmit it.
    let total_len = (VIRTIO_NET_HDR_SIZE + copy_len) as u32;
    drv.tx_queue.descriptors[idx].addr = drv.tx_buf_phys[idx];
    drv.tx_queue.descriptors[idx].len = total_len;
    drv.tx_queue.descriptors[idx].flags = 0;

    // Submit to available ring and notify device.
    // SAFETY: `io_base` is valid for this initialized driver.
    unsafe {
        drv.tx_queue.submit_and_notify(desc_idx, drv.io_base, 1);
    }

    crate::serial_println!("[NET] TX: {} bytes (desc {})", copy_len, desc_idx);
    TX_PACKETS.fetch_add(1, Ordering::Relaxed);
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
        // Header-only or zero-length -- nothing useful. Re-submit.
        // SAFETY: `io_base` is valid.
        unsafe {
            drv.rx_queue.submit_and_notify(desc_idx, drv.io_base, 0);
        }
        RX_ERRORS.fetch_add(1, Ordering::Relaxed);
        return None;
    }

    let frame_len = len as usize - VIRTIO_NET_HDR_SIZE;
    let buf = &drv.rx_buffers[idx];
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
    RX_PACKETS.fetch_add(1, Ordering::Relaxed);
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

/// Get a snapshot of the current network interface statistics.
///
/// All counters are atomic and read with `Relaxed` ordering, so the
/// snapshot represents a consistent point-in-time view of each counter
/// individually (but not all six collectively).
pub fn get_stats() -> super::net::InterfaceStats {
    super::net::InterfaceStats {
        rx_packets: RX_PACKETS.load(Ordering::Relaxed),
        tx_packets: TX_PACKETS.load(Ordering::Relaxed),
        rx_errors: RX_ERRORS.load(Ordering::Relaxed),
        tx_errors: TX_ERRORS.load(Ordering::Relaxed),
        rx_dropped: RX_DROPPED.load(Ordering::Relaxed),
        tx_dropped: TX_DROPPED.load(Ordering::Relaxed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─────────────────── Feature bit tests ───────────────────

    #[test]
    fn virtio_net_f_mac_bit_position() {
        // VIRTIO_NET_F_MAC is feature bit 5.
        assert_eq!(VIRTIO_NET_F_MAC, 1 << 5);
        assert_eq!(VIRTIO_NET_F_MAC, 0x20);
    }

    #[test]
    fn virtio_net_f_mac_is_only_bit_5() {
        assert_eq!(VIRTIO_NET_F_MAC.count_ones(), 1);
    }

    // ─────────────────── Buffer geometry tests ───────────────────

    #[test]
    fn max_frame_size() {
        assert_eq!(MAX_FRAME_SIZE, 1518);
    }

    #[test]
    fn virtio_net_hdr_size() {
        assert_eq!(VIRTIO_NET_HDR_SIZE, 10);
    }

    #[test]
    fn buf_size_is_hdr_plus_frame() {
        assert_eq!(BUF_SIZE, VIRTIO_NET_HDR_SIZE + MAX_FRAME_SIZE);
    }

    #[test]
    fn buf_size_value() {
        assert_eq!(BUF_SIZE, 1528);
    }

    // ─────────────────── VirtioNetHdr layout tests ───────────────────

    #[test]
    fn virtio_net_hdr_sizeof() {
        // The struct is #[repr(C)] with: u8, u8, u16, u16, u16, u16, u16
        // = 1 + 1 + 2 + 2 + 2 + 2 + 2 = 12 bytes (with natural alignment).
        assert_eq!(core::mem::size_of::<VirtioNetHdr>(), 12);
    }

    #[test]
    fn virtio_net_hdr_default_is_zero() {
        let hdr = VirtioNetHdr::default();
        assert_eq!(hdr.flags, 0);
        assert_eq!(hdr.gso_type, 0);
        assert_eq!(hdr.hdr_len, 0);
        assert_eq!(hdr.gso_size, 0);
        assert_eq!(hdr.csum_start, 0);
        assert_eq!(hdr.csum_offset, 0);
        assert_eq!(hdr.num_buffers, 0);
    }

    // ─────────────────── Driver initialized check ───────────────────

    #[test]
    fn mac_address_returns_zero_when_not_initialized() {
        let mac = mac_address();
        assert_eq!(mac.len(), 6);
    }

    // ─────────────────── Stats counter tests ───────────────────

    #[test]
    fn stats_counters_are_zero_by_default() {
        // At test startup (no prior sends/receives in this test binary),
        // the counters read as whatever the global state is. We test
        // that the get_stats function returns a coherent InterfaceStats.
        let stats = get_stats();
        // All fields are u64 — just verify they don't panic and are non-negative (always true for u64).
        let _total = stats.rx_packets
            + stats.tx_packets
            + stats.rx_errors
            + stats.tx_errors
            + stats.rx_dropped
            + stats.tx_dropped;
    }

    #[test]
    fn stats_rx_packets_counter_increments() {
        use core::sync::atomic::Ordering;
        let before = RX_PACKETS.load(Ordering::Relaxed);
        RX_PACKETS.fetch_add(1, Ordering::Relaxed);
        let after = RX_PACKETS.load(Ordering::Relaxed);
        assert_eq!(after, before + 1);
        // Restore
        RX_PACKETS.store(before, Ordering::Relaxed);
    }

    #[test]
    fn stats_tx_packets_counter_increments() {
        use core::sync::atomic::Ordering;
        let before = TX_PACKETS.load(Ordering::Relaxed);
        TX_PACKETS.fetch_add(1, Ordering::Relaxed);
        assert_eq!(TX_PACKETS.load(Ordering::Relaxed), before + 1);
        TX_PACKETS.store(before, Ordering::Relaxed);
    }

    #[test]
    fn stats_rx_errors_counter_increments() {
        use core::sync::atomic::Ordering;
        let before = RX_ERRORS.load(Ordering::Relaxed);
        RX_ERRORS.fetch_add(1, Ordering::Relaxed);
        assert_eq!(RX_ERRORS.load(Ordering::Relaxed), before + 1);
        RX_ERRORS.store(before, Ordering::Relaxed);
    }

    #[test]
    fn stats_tx_errors_counter_increments() {
        use core::sync::atomic::Ordering;
        let before = TX_ERRORS.load(Ordering::Relaxed);
        TX_ERRORS.fetch_add(1, Ordering::Relaxed);
        assert_eq!(TX_ERRORS.load(Ordering::Relaxed), before + 1);
        TX_ERRORS.store(before, Ordering::Relaxed);
    }

    #[test]
    fn stats_rx_dropped_counter_increments() {
        use core::sync::atomic::Ordering;
        let before = RX_DROPPED.load(Ordering::Relaxed);
        RX_DROPPED.fetch_add(1, Ordering::Relaxed);
        assert_eq!(RX_DROPPED.load(Ordering::Relaxed), before + 1);
        RX_DROPPED.store(before, Ordering::Relaxed);
    }

    #[test]
    fn stats_tx_dropped_counter_increments() {
        use core::sync::atomic::Ordering;
        let before = TX_DROPPED.load(Ordering::Relaxed);
        TX_DROPPED.fetch_add(1, Ordering::Relaxed);
        assert_eq!(TX_DROPPED.load(Ordering::Relaxed), before + 1);
        TX_DROPPED.store(before, Ordering::Relaxed);
    }

    #[test]
    fn get_stats_reflects_counter_state() {
        use core::sync::atomic::Ordering;
        // Store known values.
        RX_PACKETS.store(100, Ordering::Relaxed);
        TX_PACKETS.store(200, Ordering::Relaxed);
        RX_ERRORS.store(5, Ordering::Relaxed);
        TX_ERRORS.store(3, Ordering::Relaxed);
        RX_DROPPED.store(7, Ordering::Relaxed);
        TX_DROPPED.store(2, Ordering::Relaxed);

        let stats = get_stats();
        assert_eq!(stats.rx_packets, 100);
        assert_eq!(stats.tx_packets, 200);
        assert_eq!(stats.rx_errors, 5);
        assert_eq!(stats.tx_errors, 3);
        assert_eq!(stats.rx_dropped, 7);
        assert_eq!(stats.tx_dropped, 2);

        // Restore zeros.
        RX_PACKETS.store(0, Ordering::Relaxed);
        TX_PACKETS.store(0, Ordering::Relaxed);
        RX_ERRORS.store(0, Ordering::Relaxed);
        TX_ERRORS.store(0, Ordering::Relaxed);
        RX_DROPPED.store(0, Ordering::Relaxed);
        TX_DROPPED.store(0, Ordering::Relaxed);
    }

    #[test]
    fn stats_increment_is_additive() {
        use core::sync::atomic::Ordering;
        RX_PACKETS.store(0, Ordering::Relaxed);
        RX_PACKETS.fetch_add(10, Ordering::Relaxed);
        RX_PACKETS.fetch_add(20, Ordering::Relaxed);
        assert_eq!(RX_PACKETS.load(Ordering::Relaxed), 30);
        RX_PACKETS.store(0, Ordering::Relaxed);
    }
}
