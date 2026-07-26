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

/// Network interface statistics.
#[derive(Debug, Clone, Copy, Default)]
pub struct InterfaceStats {
    /// Total received packets.
    pub rx_packets: u64,
    /// Total transmitted packets.
    pub tx_packets: u64,
    /// Receive errors.
    pub rx_errors: u64,
    /// Transmit errors.
    pub tx_errors: u64,
    /// Received packets dropped.
    pub rx_dropped: u64,
    /// Transmitted packets dropped.
    pub tx_dropped: u64,
}

/// Get network interface statistics.
///
/// Returns a snapshot of the atomic counters maintained by the virtio-net
/// driver. Each counter is independently atomic; the snapshot is not
/// guaranteed to be globally consistent across all six fields.
#[must_use]
pub fn interface_stats() -> InterfaceStats {
    virtio_net::get_stats()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interface_stats_returns_non_panic_values() {
        let stats = interface_stats();
        // Verify all fields are accessible without panic.
        let _ = stats.rx_packets;
        let _ = stats.tx_packets;
        let _ = stats.rx_errors;
        let _ = stats.tx_errors;
        let _ = stats.rx_dropped;
        let _ = stats.tx_dropped;
    }

    #[test]
    fn interface_stats_returns_default_when_no_traffic() {
        let stats = interface_stats();
        // Without any traffic in the test process, all counters should be zero
        // (the virtio-net driver was never initialized in test mode).
        assert_eq!(stats.rx_packets, 0);
        assert_eq!(stats.tx_packets, 0);
        assert_eq!(stats.rx_errors, 0);
        assert_eq!(stats.tx_errors, 0);
        assert_eq!(stats.rx_dropped, 0);
        assert_eq!(stats.tx_dropped, 0);
    }

    #[test]
    fn interface_stats_struct_is_copy() {
        let stats = interface_stats();
        let stats2 = stats;
        // Both should be identical (Copy semantics).
        assert_eq!(stats.rx_packets, stats2.rx_packets);
        assert_eq!(stats.tx_packets, stats2.tx_packets);
    }

    #[test]
    fn interface_stats_debug_format() {
        let stats = interface_stats();
        let debug_str = alloc::format!("{stats:?}");
        assert!(debug_str.contains("InterfaceStats"));
    }
}
