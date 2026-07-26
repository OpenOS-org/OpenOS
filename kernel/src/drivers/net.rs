//! Network driver interface.
//!
//! Provides a unified API for network operations. Delegates to the
//! virtio-net driver for actual packet I/O.
//!
//! The kernel provides raw packet send/receive. The TCP/IP stack
//! runs in user-space and communicates via Channel IPC.

use alloc::vec::Vec;

use super::virtio_net;

/// Maximum Ethernet frame size.
const MAX_FRAME_SIZE: usize = 1518;

/// Minimum Ethernet frame size (header only).
const MIN_FRAME_SIZE: usize = 14;

/// Initialize the network subsystem.
/// Detects and initializes the virtio-net device.
pub fn init() {
    virtio_net::init();
}

/// Send a raw Ethernet frame.
///
/// # Errors
///
/// Returns `Err(NetError::FrameTooLarge)` if `data` exceeds `MAX_FRAME_SIZE`.
/// Returns `Err(NetError::FrameTooSmall)` if `data` is smaller than `MIN_FRAME_SIZE`.
/// Returns `Err(NetError::TxQueueFull)` if the transmit queue is full.
pub fn send_frame(data: &[u8]) -> Result<usize, NetError> {
    if data.len() > MAX_FRAME_SIZE {
        return Err(NetError::FrameTooLarge);
    }
    if data.len() < MIN_FRAME_SIZE {
        return Err(NetError::FrameTooSmall);
    }
    virtio_net::send_frame(data).map_err(|_| NetError::TxQueueFull)
}

/// Receive a raw Ethernet frame (non-blocking).
#[must_use]
pub fn receive_frame() -> Option<Vec<u8>> {
    virtio_net::receive_frame()
}

/// Get the MAC address of the network interface.
#[must_use]
pub fn mac_address() -> [u8; 6] {
    virtio_net::mac_address()
}

/// Network driver errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetError {
    /// Frame exceeds maximum Ethernet frame size.
    FrameTooLarge,
    /// Frame is smaller than minimum Ethernet frame size.
    FrameTooSmall,
    /// Network device is not initialized.
    NotInitialized,
    /// Transmit queue is full.
    TxQueueFull,
}
