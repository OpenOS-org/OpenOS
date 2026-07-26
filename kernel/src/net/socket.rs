//! Socket abstraction for network communication.
//!
//! Provides a BSD-like socket interface on top of the raw network driver.
//! Each task maintains a per-task socket table (`BTreeMap<u64, Socket>`)
//! mapping socket descriptors to socket state.
//!
//! ## Socket Types
//!
//! - **Tcp** — Stream socket (connection-oriented, reliable). Not yet implemented.
//! - **Udp** — Datagram socket (connectionless). Not yet implemented.
//! - **Raw** — Raw Ethernet/IP socket. Not yet implemented.
//!
//! All socket operations currently return `NotImplemented` until the
//! TCP/UDP protocol layers are built out.

use alloc::collections::BTreeMap;

/// Socket types supported by the kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketType {
    /// TCP stream socket (connection-oriented).
    Tcp,
    /// UDP datagram socket (connectionless).
    Udp,
    /// Raw IP/Ethernet socket.
    Raw,
}

/// State of a socket connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketState {
    /// Just created, not yet bound or connected.
    Created,
    /// Bound to a local address/port.
    Bound,
    /// Listening for incoming connections (TCP only).
    Listening,
    /// Connected to a remote peer (TCP only).
    Connected,
    /// Socket has been closed.
    Closed,
}

/// A network socket.
///
/// Represents one endpoint of a network connection. Holds the socket type,
/// current state, and addressing information.
#[derive(Debug, Clone)]
pub struct Socket {
    /// Type of socket (TCP, UDP, or Raw).
    #[allow(clippy::struct_field_names)]
    pub socket_type: SocketType,
    /// Current connection state.
    pub state: SocketState,
    /// Local port number (0 if not yet bound).
    pub local_port: u16,
    /// Remote IPv4 address (network byte order, 0 if not connected).
    pub remote_addr: u32,
    /// Remote port number (0 if not connected).
    pub remote_port: u16,
}

impl Socket {
    /// Create a new socket in the `Created` state.
    #[must_use]
    pub fn new(socket_type: SocketType) -> Self {
        Self {
            socket_type,
            state: SocketState::Created,
            local_port: 0,
            remote_addr: 0,
            remote_port: 0,
        }
    }
}

/// Per-task socket table.
///
/// Maps socket descriptors (small integers) to `Socket` instances.
/// The descriptor space starts at 0 and increments on each `socket()` call.
pub type SocketTable = BTreeMap<u64, Socket>;
