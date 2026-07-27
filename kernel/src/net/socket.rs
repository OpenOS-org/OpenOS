//! Socket abstraction for network communication.
//!
//! Provides a BSD-like socket interface on top of the raw network driver.
//! Each task maintains a per-task socket table (`BTreeMap<u64, Socket>`)
//! mapping socket descriptors to socket state.
//!
//! ## Socket Types
//!
//! - **Tcp** — Stream socket (connection-oriented, reliable).
//! - **Udp** — Datagram socket (connectionless).
//! - **Raw** — Raw Ethernet/IP socket. Not yet implemented.

use alloc::collections::BTreeMap;

/// Socket option level constants (match Linux/POSIX values).
pub const SOL_SOCKET: u32 = 1;
/// Socket option level for TCP.
pub const IPPROTO_TCP: u32 = 6;

/// Socket option name: allow reuse of local addresses.
pub const SO_REUSEADDR: u32 = 2;
/// TCP option name: disable Nagle's algorithm.
pub const TCP_NODELAY: u32 = 1;
/// Socket option name: receive buffer size.
pub const SO_RCVBUF: u32 = 8;
/// Socket option name: send buffer size.
pub const SO_SNDBUF: u32 = 7;

/// Configurable options for a socket.
///
/// Stores per-socket tuning parameters that user-space can read and write
/// via `getsockopt` / `setsockopt`.
#[derive(Debug, Clone)]
pub struct SocketOptions {
    /// Allow binding to an address that is already in use.
    pub reuse_addr: bool,
    /// Disable Nagle's algorithm (`TCP_NODELAY`).
    pub tcp_no_delay: bool,
    /// Receive buffer size in bytes.
    pub rcv_buf_size: u32,
    /// Send buffer size in bytes.
    pub snd_buf_size: u32,
}

impl Default for SocketOptions {
    fn default() -> Self {
        Self {
            reuse_addr: false,
            tcp_no_delay: false,
            rcv_buf_size: 8192,
            snd_buf_size: 8192,
        }
    }
}

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
    /// Configurable socket options.
    pub options: SocketOptions,
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
            options: SocketOptions::default(),
        }
    }
}

/// Per-task socket table.
///
/// Maps socket descriptors (small integers) to `Socket` instances.
/// The descriptor space starts at 0 and increments on each `socket()` call.
pub type SocketTable = BTreeMap<u64, Socket>;

#[cfg(test)]
mod tests {
    extern crate alloc;

    use alloc::collections::BTreeMap;

    use super::*;

    // ─────────────────── SocketType enum tests ───────────────────

    #[test]
    fn socket_type_variants_exist() {
        let tcp = SocketType::Tcp;
        let udp = SocketType::Udp;
        let raw = SocketType::Raw;
        assert_ne!(tcp, udp);
        assert_ne!(tcp, raw);
        assert_ne!(udp, raw);
    }

    #[test]
    fn socket_type_clone() {
        let original = SocketType::Udp;
        let cloned = original;
        assert_eq!(original, cloned);
    }

    #[test]
    fn socket_type_debug() {
        let tcp = SocketType::Tcp;
        let debug_str = alloc::format!("{:?}", tcp);
        assert_eq!(debug_str, "Tcp");
    }

    #[test]
    fn socket_type_copy() {
        let a = SocketType::Raw;
        let b = a; // Copy, not move.
        assert_eq!(a, b); // 'a' is still usable.
    }

    // ─────────────────── SocketState enum tests ───────────────────

    #[test]
    fn socket_state_variants_exist() {
        let states = [
            SocketState::Created,
            SocketState::Bound,
            SocketState::Listening,
            SocketState::Connected,
            SocketState::Closed,
        ];
        // All variants should be distinct.
        for i in 0..states.len() {
            for j in (i + 1)..states.len() {
                assert_ne!(states[i], states[j]);
            }
        }
    }

    #[test]
    fn socket_state_debug() {
        assert_eq!(alloc::format!("{:?}", SocketState::Created), "Created");
        assert_eq!(alloc::format!("{:?}", SocketState::Bound), "Bound");
        assert_eq!(alloc::format!("{:?}", SocketState::Listening), "Listening");
        assert_eq!(alloc::format!("{:?}", SocketState::Connected), "Connected");
        assert_eq!(alloc::format!("{:?}", SocketState::Closed), "Closed");
    }

    #[test]
    fn socket_state_clone() {
        let original = SocketState::Connected;
        let cloned = original;
        assert_eq!(original, cloned);
    }

    #[test]
    fn socket_state_copy() {
        let a = SocketState::Bound;
        let b = a;
        assert_eq!(a, b);
    }

    // ─────────────────── Socket creation tests ───────────────────

    #[test]
    fn socket_new_tcp() {
        let sock = Socket::new(SocketType::Tcp);
        assert_eq!(sock.socket_type, SocketType::Tcp);
        assert_eq!(sock.state, SocketState::Created);
        assert_eq!(sock.local_port, 0);
        assert_eq!(sock.remote_addr, 0);
        assert_eq!(sock.remote_port, 0);
    }

    #[test]
    fn socket_new_udp() {
        let sock = Socket::new(SocketType::Udp);
        assert_eq!(sock.socket_type, SocketType::Udp);
        assert_eq!(sock.state, SocketState::Created);
    }

    #[test]
    fn socket_new_raw() {
        let sock = Socket::new(SocketType::Raw);
        assert_eq!(sock.socket_type, SocketType::Raw);
        assert_eq!(sock.state, SocketState::Created);
    }

    #[test]
    fn socket_default_state_is_created() {
        for socket_type in [SocketType::Tcp, SocketType::Udp, SocketType::Raw] {
            let sock = Socket::new(socket_type);
            assert_eq!(sock.state, SocketState::Created);
        }
    }

    // ─────────────────── Socket state transitions ───────────────────

    #[test]
    fn socket_state_created_to_bound() {
        let mut sock = Socket::new(SocketType::Udp);
        assert_eq!(sock.state, SocketState::Created);
        sock.state = SocketState::Bound;
        sock.local_port = 8080;
        assert_eq!(sock.state, SocketState::Bound);
        assert_eq!(sock.local_port, 8080);
    }

    #[test]
    fn socket_state_bound_to_listening() {
        let mut sock = Socket::new(SocketType::Tcp);
        sock.state = SocketState::Bound;
        sock.local_port = 80;
        sock.state = SocketState::Listening;
        assert_eq!(sock.state, SocketState::Listening);
    }

    #[test]
    fn socket_state_created_to_connected() {
        let mut sock = Socket::new(SocketType::Tcp);
        sock.state = SocketState::Connected;
        sock.remote_addr = 0x0100A8C0; // 192.168.0.1
        sock.remote_port = 80;
        assert_eq!(sock.state, SocketState::Connected);
        assert_eq!(sock.remote_addr, 0x0100A8C0);
        assert_eq!(sock.remote_port, 80);
    }

    #[test]
    fn socket_state_to_closed() {
        let mut sock = Socket::new(SocketType::Tcp);
        sock.state = SocketState::Connected;
        sock.state = SocketState::Closed;
        assert_eq!(sock.state, SocketState::Closed);
    }

    #[test]
    fn socket_state_full_lifecycle() {
        let mut sock = Socket::new(SocketType::Tcp);
        assert_eq!(sock.state, SocketState::Created);

        sock.state = SocketState::Bound;
        sock.local_port = 443;
        assert_eq!(sock.state, SocketState::Bound);

        sock.state = SocketState::Listening;
        assert_eq!(sock.state, SocketState::Listening);

        sock.state = SocketState::Connected;
        sock.remote_addr = 0x08080808; // 8.8.8.8
        sock.remote_port = 443;
        assert_eq!(sock.state, SocketState::Connected);

        sock.state = SocketState::Closed;
        assert_eq!(sock.state, SocketState::Closed);
    }

    // ─────────────────── Socket field tests ───────────────────

    #[test]
    fn socket_local_port_field() {
        let mut sock = Socket::new(SocketType::Udp);
        sock.local_port = 68;
        assert_eq!(sock.local_port, 68);
    }

    #[test]
    fn socket_remote_addr_field() {
        let mut sock = Socket::new(SocketType::Tcp);
        sock.remote_addr = 0xC0A80001; // 192.168.0.1 in big-endian
        assert_eq!(sock.remote_addr, 0xC0A80001);
    }

    #[test]
    fn socket_remote_port_field() {
        let mut sock = Socket::new(SocketType::Tcp);
        sock.remote_port = 80;
        assert_eq!(sock.remote_port, 80);
    }

    #[test]
    fn socket_clone() {
        let mut sock = Socket::new(SocketType::Udp);
        sock.local_port = 53;
        sock.remote_addr = 0x08080808;
        sock.remote_port = 53;

        let cloned = sock.clone();
        assert_eq!(cloned.socket_type, SocketType::Udp);
        assert_eq!(cloned.local_port, 53);
        assert_eq!(cloned.remote_addr, 0x08080808);
        assert_eq!(cloned.remote_port, 53);
    }

    #[test]
    fn socket_debug_format() {
        let sock = Socket::new(SocketType::Tcp);
        let debug = alloc::format!("{:?}", sock);
        assert!(debug.contains("Tcp"));
        assert!(debug.contains("Created"));
    }

    // ─────────────────── SocketTable tests ───────────────────

    #[test]
    fn socket_table_is_btree_map() {
        let mut table: SocketTable = BTreeMap::new();
        assert!(table.is_empty());

        table.insert(0, Socket::new(SocketType::Tcp));
        table.insert(1, Socket::new(SocketType::Udp));

        assert_eq!(table.len(), 2);
        assert!(table.contains_key(&0));
        assert!(table.contains_key(&1));
    }

    #[test]
    fn socket_table_insert_and_lookup() {
        let mut table: SocketTable = BTreeMap::new();
        let fd = 42u64;
        table.insert(fd, Socket::new(SocketType::Raw));

        let sock = table.get(&fd).unwrap();
        assert_eq!(sock.socket_type, SocketType::Raw);
        assert_eq!(sock.state, SocketState::Created);
    }

    #[test]
    fn socket_table_remove() {
        let mut table: SocketTable = BTreeMap::new();
        table.insert(0, Socket::new(SocketType::Tcp));
        assert_eq!(table.len(), 1);

        table.remove(&0);
        assert!(table.is_empty());
    }

    #[test]
    fn socket_table_multiple_sockets() {
        let mut table: SocketTable = BTreeMap::new();
        for i in 0..10u64 {
            table.insert(i, Socket::new(SocketType::Udp));
        }
        assert_eq!(table.len(), 10);

        // Verify all are retrievable.
        for i in 0..10u64 {
            assert!(table.contains_key(&i));
        }
    }

    // ─────────────────── SocketOptions tests ───────────────────

    #[test]
    fn socket_options_default() {
        let opts = SocketOptions::default();
        assert!(!opts.reuse_addr);
        assert!(!opts.tcp_no_delay);
        assert_eq!(opts.rcv_buf_size, 8192);
        assert_eq!(opts.snd_buf_size, 8192);
    }

    #[test]
    fn socket_options_clone() {
        let mut opts = SocketOptions::default();
        opts.reuse_addr = true;
        opts.tcp_no_delay = true;
        opts.rcv_buf_size = 16384;
        opts.snd_buf_size = 4096;

        let cloned = opts.clone();
        assert!(cloned.reuse_addr);
        assert!(cloned.tcp_no_delay);
        assert_eq!(cloned.rcv_buf_size, 16384);
        assert_eq!(cloned.snd_buf_size, 4096);
    }

    #[test]
    fn socket_options_debug() {
        let opts = SocketOptions::default();
        let debug = alloc::format!("{:?}", opts);
        assert!(debug.contains("reuse_addr"));
        assert!(debug.contains("tcp_no_delay"));
        assert!(debug.contains("rcv_buf_size"));
        assert!(debug.contains("snd_buf_size"));
    }

    #[test]
    fn socket_options_on_new_socket() {
        let sock = Socket::new(SocketType::Tcp);
        assert!(!sock.options.reuse_addr);
        assert!(!sock.options.tcp_no_delay);
        assert_eq!(sock.options.rcv_buf_size, 8192);
        assert_eq!(sock.options.snd_buf_size, 8192);
    }

    #[test]
    fn socket_options_mutable() {
        let mut sock = Socket::new(SocketType::Tcp);
        sock.options.reuse_addr = true;
        sock.options.tcp_no_delay = true;
        sock.options.rcv_buf_size = 32768;
        sock.options.snd_buf_size = 32768;

        assert!(sock.options.reuse_addr);
        assert!(sock.options.tcp_no_delay);
        assert_eq!(sock.options.rcv_buf_size, 32768);
        assert_eq!(sock.options.snd_buf_size, 32768);
    }

    #[test]
    fn socket_options_level_constants() {
        assert_eq!(SOL_SOCKET, 1);
        assert_eq!(IPPROTO_TCP, 6);
    }

    #[test]
    fn socket_option_name_constants() {
        assert_eq!(SO_REUSEADDR, 2);
        assert_eq!(TCP_NODELAY, 1);
        assert_eq!(SO_RCVBUF, 8);
        assert_eq!(SO_SNDBUF, 7);
    }
}
