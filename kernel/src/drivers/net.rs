//! Network driver skeleton — virtio-net placeholder.
//!
//! Provides the foundation for user-space networking. The actual TCP/IP
//! stack runs as a user-space service; the kernel driver only handles
//! raw packet I/O via virtio-net.
//!
//! ## Design
//!
//! - Kernel provides: raw packet send/receive via virtio-net MMIO
//! - User-space provides: TCP/IP stack, socket API, DNS
//! - Communication: Channel IPC between kernel driver and user-space network server
//!
//! ## Status
//!
//! Skeleton only — virtio-net device detection and MMIO mapping are stubs.
//! Full implementation requires:
//! 1. PCI device enumeration for virtio-net
//! 2. Virtqueue setup (rx/tx queues)
//! 3. Interrupt-driven packet reception
//! 4. DMA buffer management

use alloc::vec::Vec;

use spin::Mutex;

/// Maximum Ethernet frame size.
const MAX_FRAME_SIZE: usize = 1518;

/// Network driver state.
struct NetDriver {
    /// Whether the driver has been initialized.
    initialized: bool,
    /// MAC address of the network interface.
    mac: [u8; 6],
    /// Received packets buffer.
    rx_buffer: Vec<Vec<u8>>,
}

static NET_DRIVER: Mutex<NetDriver> = Mutex::new(NetDriver {
    initialized: false,
    mac: [0; 6],
    rx_buffer: Vec::new(),
});

/// Initialize the network driver.
/// Currently a no-op — virtio-net device detection is not implemented.
pub fn init() {
    crate::serial_println!("[OK] Network driver initialized (skeleton)");
}

/// Send a raw Ethernet frame.
/// Returns `Ok(bytes_sent)` on success, `Err` on failure.
pub fn send_frame(data: &[u8]) -> Result<usize, NetError> {
    if data.len() > MAX_FRAME_SIZE {
        return Err(NetError::FrameTooLarge);
    }
    if data.len() < 14 {
        return Err(NetError::FrameTooSmall);
    }

    // TODO: Send via virtio-net tx virtqueue.
    crate::serial_println!("[NET] send_frame: {} bytes (stub)", data.len());
    Ok(data.len())
}

/// Receive a raw Ethernet frame (non-blocking).
/// Returns the frame data if available, or None.
pub fn receive_frame() -> Option<Vec<u8>> {
    let mut driver = NET_DRIVER.lock();
    driver.rx_buffer.pop()
}

/// Get the MAC address of the network interface.
pub fn mac_address() -> [u8; 6] {
    NET_DRIVER.lock().mac
}

/// Errors that can occur in the network driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetError {
    /// Frame exceeds maximum Ethernet frame size.
    FrameTooLarge,
    /// Frame is too small to be a valid Ethernet frame.
    FrameTooSmall,
    /// Network device not initialized.
    NotInitialized,
    /// Transmit queue full.
    TxQueueFull,
}
