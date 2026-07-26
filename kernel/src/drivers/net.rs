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
pub fn receive_frame() -> Option<Vec<u8>> {
    virtio_net::receive_frame()
}

/// Get the MAC address of the network interface.
pub fn mac_address() -> [u8; 6] {
    virtio_net::mac_address()
}

/// Network driver errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetError {
    FrameTooLarge,
    FrameTooSmall,
    NotInitialized,
    TxQueueFull,
}
