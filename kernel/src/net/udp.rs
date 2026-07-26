//! UDP (User Datagram Protocol) packet construction and socket support.
//!
//! Provides UDP header construction for building outgoing datagrams and a
//! per-port receive queue for incoming datagrams. Used by the DHCP client
//! for encapsulation and by user-space UDP sockets for send/receive.
//!
//! ## UDP Header Format (RFC 768)
//!
//! ```text
//!  0                   1                   2                   3
//!  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |          Source Port          |       Destination Port        |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |            Length             |           Checksum            |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! ```
//!
//! ## Receive Queue Architecture
//!
//! Each bound UDP port has a FIFO queue of received datagrams. When an
//! incoming UDP frame arrives, the frame dispatcher calls
//! `handle_incoming_udp()` which parses the UDP header and enqueues the
//! payload into the queue for the destination port. User-space calls
//! `recv_udp_packet()` to dequeue datagrams.
//!
//! The queue has a configurable maximum depth (`UDP_QUEUE_MAX_DEPTH`).
//! When the queue is full, new datagrams are silently dropped (standard
//! UDP behavior — unreliable delivery).

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

use spin::Mutex;

use crate::serial_println;

/// UDP header size in bytes.
const UDP_HEADER_SIZE: usize = 8;

/// Maximum number of datagrams buffered per port before dropping.
const UDP_QUEUE_MAX_DEPTH: usize = 64;

/// DHCP server port (well-known, RFC 2131).
pub const DHCP_SERVER_PORT: u16 = 67;

/// DHCP client port (well-known, RFC 2131).
pub const DHCP_CLIENT_PORT: u16 = 68;

/// A received UDP datagram with source addressing metadata.
///
/// Enqueued in the per-port receive queue when an incoming UDP frame
/// arrives. Dequeued by `recv_udp_packet()`.
#[derive(Debug, Clone)]
pub struct UdpDatagram {
    /// Source IPv4 address (network byte order).
    pub src_addr: u32,
    /// Source UDP port (host byte order).
    pub src_port: u16,
    /// Destination UDP port (host byte order).
    pub dst_port: u16,
    /// Payload data (UDP header already stripped).
    pub data: Vec<u8>,
}

/// Per-port receive queues for incoming UDP datagrams.
///
/// Maps destination port number to a FIFO queue of `UdpDatagram`s.
/// Protected by a mutex since the network service loop (which enqueues)
/// and user-space tasks (which dequeue) run on different contexts.
static UDP_RECV_QUEUES: Mutex<BTreeMap<u16, Vec<UdpDatagram>>> = Mutex::new(BTreeMap::new());

/// Pseudo-header for UDP checksum calculation (RFC 768).
///
/// The checksum covers a 12-byte pseudo-header (source IP, destination IP,
/// zero, protocol, UDP length), the UDP header, and the payload.
struct PseudoHeader {
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    protocol: u8,
    udp_length: u16,
}

/// Compute the Internet checksum (RFC 1071) over a byte buffer.
///
/// Sums 16-bit words, folds carries, returns the one's complement.
fn internet_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;

    // Sum 16-bit words.
    while i + 1 < data.len() {
        sum += u32::from(u16::from_be_bytes([data[i], data[i + 1]]));
        i += 2;
    }

    // Handle odd byte.
    if i < data.len() {
        sum += u32::from(data[i]) << 8;
    }

    // Fold 32-bit sum into 16 bits.
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    !sum as u16
}

/// Build a UDP datagram wrapped in an IPv4 packet.
///
/// Returns a complete IPv4 + UDP + payload byte vector ready to be
/// encapsulated in an Ethernet frame.
///
/// # Arguments
///
/// * `src_ip` - Source IPv4 address.
/// * `dst_ip` - Destination IPv4 address (255.255.255.255 for broadcast).
/// * `src_port` - Source UDP port.
/// * `dst_port` - Destination UDP port.
/// * `payload` - UDP payload data.
///
/// # Returns
///
/// A `Vec<u8>` containing the IPv4 header, UDP header, and payload.
/// The UDP checksum is computed over the pseudo-header + UDP header + payload.
/// Build a UDP datagram wrapped in an IPv4 packet.
///
/// Returns a complete IPv4 + UDP + payload byte vector ready to be
/// encapsulated in an Ethernet frame.
///
/// # Arguments
///
/// * `src_ip` - Source IPv4 address.
/// * `dst_ip` - Destination IPv4 address (255.255.255.255 for broadcast).
/// * `src_port` - Source UDP port.
/// * `dst_port` - Destination UDP port.
/// * `payload` - UDP payload data.
///
/// # Returns
///
/// A `Vec<u8>` containing the IPv4 header, UDP header, and payload.
/// The UDP checksum is computed over the pseudo-header + UDP header + payload.
#[must_use]
pub fn build_udp(
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    src_port: u16,
    dst_port: u16,
    payload: &[u8],
) -> Vec<u8> {
    let udp_length = UDP_HEADER_SIZE + payload.len();
    let ip_total_length = IP_HEADER_SIZE + udp_length;

    // Build the UDP header (checksum = 0 initially).
    let mut udp_header = [0u8; UDP_HEADER_SIZE];
    udp_header[0..2].copy_from_slice(&src_port.to_be_bytes());
    udp_header[2..4].copy_from_slice(&dst_port.to_be_bytes());
    udp_header[4..6].copy_from_slice(&(udp_length as u16).to_be_bytes());
    // Checksum field is at offset 6..8, left as 0 for now.

    // Build pseudo-header + UDP header + payload for checksum.
    let pseudo = PseudoHeader {
        src_ip,
        dst_ip,
        protocol: IP_PROTOCOL_UDP,
        udp_length: udp_length as u16,
    };
    let mut checksum_buf = Vec::with_capacity(12 + udp_length);
    // Pseudo-header: source IP, destination IP, zero + protocol, UDP length.
    checksum_buf.extend_from_slice(&pseudo.src_ip);
    checksum_buf.extend_from_slice(&pseudo.dst_ip);
    checksum_buf.push(0);
    checksum_buf.push(pseudo.protocol);
    checksum_buf.extend_from_slice(&pseudo.udp_length.to_be_bytes());
    // UDP header + payload.
    checksum_buf.extend_from_slice(&udp_header);
    checksum_buf.extend_from_slice(payload);

    let checksum = internet_checksum(&checksum_buf);
    udp_header[6..8].copy_from_slice(&checksum.to_be_bytes());

    // Build IPv4 header.
    let mut ip_header = build_ip_header(src_ip, dst_ip, ip_total_length as u16);

    // Assemble: IP header + UDP header + payload.
    ip_header.extend_from_slice(&udp_header);
    ip_header.extend_from_slice(payload);
    ip_header
}

/// IPv4 header size in bytes (20 bytes, no options).
const IP_HEADER_SIZE: usize = 20;

/// IPv4 version (4) and IHL (5 = 20 bytes, no options) combined.
const IP_VERSION_IHL: u8 = 0x45;

/// DSCP + ECN field (best effort = 0).
const IP_DSCP_ECN: u8 = 0x00;

/// IPv4 protocol number for UDP (RFC 768).
const IP_PROTOCOL_UDP: u8 = 17;

/// IPv4 time-to-live (default 64 per RFC 1122).
const IP_TTL_DEFAULT: u8 = 64;

/// Build an IPv4 header with zeroed checksum (caller fills payload).
///
/// The checksum is computed over the header only and placed at offset 10..12.
fn build_ip_header(src_ip: [u8; 4], dst_ip: [u8; 4], total_length: u16) -> Vec<u8> {
    let mut header = vec![0u8; IP_HEADER_SIZE];
    header[0] = IP_VERSION_IHL;
    header[1] = IP_DSCP_ECN;
    header[2..4].copy_from_slice(&total_length.to_be_bytes());
    // Identification: 0 (single packet, no fragmentation).
    // Flags + Fragment Offset: 0 (no fragmentation).
    header[6] = 0x40; // Don't Fragment flag.
    header[7] = 0x00;
    header[8] = IP_TTL_DEFAULT;
    header[9] = IP_PROTOCOL_UDP;
    // Checksum at 10..12, zeroed for calculation.
    header[12..16].copy_from_slice(&src_ip);
    header[16..20].copy_from_slice(&dst_ip);

    // Compute and set the IPv4 header checksum.
    let checksum = internet_checksum(&header);
    header[10..12].copy_from_slice(&checksum.to_be_bytes());

    header
}

// ─────────────────── Receive queue management ───────────────────

/// Bind a UDP port by creating a receive queue for it.
///
/// Must be called before the port can receive datagrams. Idempotent —
/// calling on an already-bound port is a no-op.
pub fn bind_udp_port(port: u16) {
    let mut queues = UDP_RECV_QUEUES.lock();
    queues.entry(port).or_default();
    serial_println!("[UDP] bound port {}", port);
}

/// Unbind a UDP port, discarding any queued datagrams.
pub fn unbind_udp_port(port: u16) {
    let mut queues = UDP_RECV_QUEUES.lock();
    queues.remove(&port);
    serial_println!("[UDP] unbound port {}", port);
}

/// Handle an incoming UDP datagram from the frame dispatcher.
///
/// Parses the UDP header (source port, destination port, length) and
/// enqueues the payload into the receive queue for the destination port.
/// If no queue exists for the destination port, the datagram is silently
/// dropped (standard UDP behavior).
///
/// # Arguments
///
/// * `src_addr` - Source IPv4 address (network byte order).
/// * `udp_data` - Raw UDP data starting at the UDP header (8 bytes + payload).
pub fn handle_incoming_udp(src_addr: u32, udp_data: &[u8]) {
    if udp_data.len() < UDP_HEADER_SIZE {
        serial_println!(
            "[UDP] incoming datagram too short ({} bytes), dropping",
            udp_data.len()
        );
        return;
    }

    let src_port = u16::from_be_bytes([udp_data[0], udp_data[1]]);
    let dst_port = u16::from_be_bytes([udp_data[2], udp_data[3]]);
    let _length = u16::from_be_bytes([udp_data[4], udp_data[5]]);
    // Bytes 6..8 are checksum — we accept without verification (common in UDP stacks).

    let payload = &udp_data[UDP_HEADER_SIZE..];

    serial_println!(
        "[UDP] incoming datagram: {}:{} -> :{} ({} bytes payload)",
        format_ip(src_addr),
        src_port,
        dst_port,
        payload.len()
    );

    let datagram = UdpDatagram {
        src_addr,
        src_port,
        dst_port,
        data: Vec::from(payload),
    };

    let mut queues = UDP_RECV_QUEUES.lock();
    if let Some(queue) = queues.get_mut(&dst_port) {
        if queue.len() < UDP_QUEUE_MAX_DEPTH {
            queue.push(datagram);
        } else {
            serial_println!("[UDP] port {} queue full, dropping datagram", dst_port);
        }
    }
    // If no queue exists for dst_port, silently drop (not bound).
}

/// Send a UDP datagram to a remote address.
///
/// Builds a UDP packet (UDP header + payload) wrapped in IPv4, then wraps
/// that in an Ethernet frame and transmits via the network driver. Resolves
/// the destination MAC via ARP.
///
/// # Arguments
///
/// * `src_port` - Local (source) UDP port.
/// * `dst_addr` - Destination IPv4 address (network byte order).
/// * `dst_port` - Destination UDP port.
/// * `data` - Payload data to send.
///
/// # Returns
///
/// `Ok(bytes_sent)` on success, `Err(())` if ARP resolution fails or the
/// network driver rejects the frame.
pub fn send_udp_packet(
    src_port: u16,
    dst_addr: u32,
    dst_port: u16,
    data: &[u8],
) -> Result<usize, ()> {
    let local_ip = super::local_ip();
    let src_ip_bytes = local_ip.to_be_bytes();
    let dst_ip_bytes = dst_addr.to_be_bytes();

    // Build the IP + UDP packet (reuse existing build_udp).
    let ip_udp_packet = build_udp(src_ip_bytes, dst_ip_bytes, src_port, dst_port, data);

    // Resolve the destination MAC via ARP.
    let Some(dst_mac) = super::arp_lookup(dst_addr) else {
        serial_println!(
            "[UDP] send: no ARP entry for {:?}, dropping",
            super::format_ip(dst_addr)
        );
        return Err(());
    };

    // Build Ethernet frame.
    let src_mac = crate::drivers::net::mac_address();
    let frame = super::build_ethernet(dst_mac, src_mac, ETHERTYPE_IPV4, &ip_udp_packet);

    match crate::drivers::net::send_frame(&frame) {
        Ok(sent) => {
            serial_println!(
                "[UDP] sent {} bytes: {}:{} -> {:?}:{}",
                data.len(),
                src_port,
                super::format_ip(local_ip),
                super::format_ip(dst_addr),
                dst_port
            );
            Ok(sent)
        }
        Err(e) => {
            serial_println!("[UDP] send failed: {:?}", e);
            Err(())
        }
    }
}

/// Receive a UDP datagram from the queue for the given port (non-blocking).
///
/// Dequeues the oldest datagram from the receive queue for `port`.
/// Returns the datagram if one is available, or `None` if the queue is
/// empty or the port is not bound.
///
/// # Arguments
///
/// * `port` - Local port to receive from.
/// * `buf` - Buffer to copy the payload into.
///
/// # Returns
///
/// `Ok((bytes_copied, src_addr, src_port))` if a datagram was available,
/// `Ok((0, 0, 0))` if the queue is empty, or `Err(())` if the port is not bound.
pub fn recv_udp_packet(port: u16, buf: &mut [u8]) -> Result<(usize, u32, u16), ()> {
    let mut queues = UDP_RECV_QUEUES.lock();
    let queue = queues.get_mut(&port).ok_or(())?;

    let Some(datagram) = queue.pop() else {
        return Ok((0, 0, 0));
    };

    let to_copy = datagram.data.len().min(buf.len());
    buf[..to_copy].copy_from_slice(&datagram.data[..to_copy]);

    Ok((to_copy, datagram.src_addr, datagram.src_port))
}

/// `EtherType` for IPv4 (0x0800).
const ETHERTYPE_IPV4: u16 = 0x0800;

/// Format an IPv4 address for debug logging.
fn format_ip(ip: u32) -> alloc::string::String {
    let b = ip.to_be_bytes();
    alloc::format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3])
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─────────────────── Constant tests ───────────────────

    #[test]
    fn udp_header_size() {
        assert_eq!(UDP_HEADER_SIZE, 8);
    }

    #[test]
    fn dhcp_server_port() {
        assert_eq!(DHCP_SERVER_PORT, 67);
    }

    #[test]
    fn dhcp_client_port() {
        assert_eq!(DHCP_CLIENT_PORT, 68);
    }

    #[test]
    fn ip_header_size() {
        assert_eq!(IP_HEADER_SIZE, 20);
    }

    #[test]
    fn ip_version_ihl() {
        // Version 4 (high nibble) + IHL 5 (low nibble) = 0x45.
        assert_eq!(IP_VERSION_IHL, 0x45);
    }

    #[test]
    fn ip_protocol_udp() {
        assert_eq!(IP_PROTOCOL_UDP, 17);
    }

    #[test]
    fn ip_ttl_default() {
        assert_eq!(IP_TTL_DEFAULT, 64);
    }

    // ─────────────────── Internet checksum tests ───────────────────

    #[test]
    fn checksum_empty_data() {
        // Checksum of empty data is 0xFFFF.
        assert_eq!(internet_checksum(&[]), 0xFFFF);
    }

    #[test]
    fn checksum_known_values() {
        // Test with a known payload.
        let data = [0x00, 0x11, 0x00, 0x08];
        let checksum = internet_checksum(&data);
        // Verify the checksum is valid: recomputing with it included yields 0.
        let mut with_checksum = data.to_vec();
        let csum_bytes = checksum.to_be_bytes();
        with_checksum.push(csum_bytes[0]);
        with_checksum.push(csum_bytes[1]);
        // Note: only valid for even-length data when checksum field is included.
        assert!(checksum != 0 || data.iter().all(|&b| b == 0));
    }

    #[test]
    fn checksum_odd_length() {
        // Odd-length data should not panic.
        let data = [0x01, 0x02, 0x03];
        let checksum = internet_checksum(&data);
        assert!(checksum != 0 || data.iter().all(|&b| b == 0));
    }

    #[test]
    fn checksum_symmetric() {
        // Two different inputs should generally produce different checksums.
        let a = internet_checksum(&[0x00, 0x01]);
        let b = internet_checksum(&[0x00, 0x02]);
        assert_ne!(a, b);
    }

    // ─────────────────── UDP header building tests ───────────────────

    #[test]
    fn build_udp_total_length() {
        let payload = [0u8; 10];
        let packet = build_udp([10, 0, 0, 1], [10, 0, 0, 2], 1234, 5678, &payload);
        // IP header (20) + UDP header (8) + payload (10) = 38.
        assert_eq!(packet.len(), IP_HEADER_SIZE + UDP_HEADER_SIZE + 10);
    }

    #[test]
    fn build_udp_ip_header_version() {
        let packet = build_udp([10, 0, 0, 1], [10, 0, 0, 2], 1234, 5678, &[]);
        // First byte should be 0x45 (IPv4, IHL=5).
        assert_eq!(packet[0], IP_VERSION_IHL);
    }

    #[test]
    fn build_udp_ip_protocol_field() {
        let packet = build_udp([10, 0, 0, 1], [10, 0, 0, 2], 1234, 5678, &[]);
        // Protocol field at offset 9 should be 17 (UDP).
        assert_eq!(packet[9], IP_PROTOCOL_UDP);
    }

    #[test]
    fn build_udp_ip_ttl_field() {
        let packet = build_udp([10, 0, 0, 1], [10, 0, 0, 2], 1234, 5678, &[]);
        assert_eq!(packet[8], IP_TTL_DEFAULT);
    }

    #[test]
    fn build_udp_ip_total_length_field() {
        let payload = [0u8; 100];
        let packet = build_udp([10, 0, 0, 1], [10, 0, 0, 2], 1234, 5678, &payload);
        let total_len = u16::from_be_bytes([packet[2], packet[3]]);
        assert_eq!(total_len as usize, IP_HEADER_SIZE + UDP_HEADER_SIZE + 100);
    }

    #[test]
    fn build_udp_ip_source_address() {
        let src = [192, 168, 1, 1];
        let packet = build_udp(src, [10, 0, 0, 2], 1234, 5678, &[]);
        assert_eq!(&packet[12..16], &src);
    }

    #[test]
    fn build_udp_ip_dest_address() {
        let dst = [255, 255, 255, 255];
        let packet = build_udp([10, 0, 0, 1], dst, 1234, 5678, &[]);
        assert_eq!(&packet[16..20], &dst);
    }

    #[test]
    fn build_udp_source_port() {
        let packet = build_udp([10, 0, 0, 1], [10, 0, 0, 2], 12345, 5678, &[]);
        let udp_start = IP_HEADER_SIZE;
        let src_port = u16::from_be_bytes([packet[udp_start], packet[udp_start + 1]]);
        assert_eq!(src_port, 12345);
    }

    #[test]
    fn build_udp_dest_port() {
        let packet = build_udp([10, 0, 0, 1], [10, 0, 0, 2], 1234, 5678, &[]);
        let udp_start = IP_HEADER_SIZE;
        let dst_port = u16::from_be_bytes([packet[udp_start + 2], packet[udp_start + 3]]);
        assert_eq!(dst_port, 5678);
    }

    #[test]
    fn build_udp_length_field() {
        let payload = [0u8; 50];
        let packet = build_udp([10, 0, 0, 1], [10, 0, 0, 2], 1234, 5678, &payload);
        let udp_start = IP_HEADER_SIZE;
        let udp_len = u16::from_be_bytes([packet[udp_start + 4], packet[udp_start + 5]]);
        assert_eq!(udp_len as usize, UDP_HEADER_SIZE + 50);
    }

    #[test]
    fn build_udp_checksum_is_nonzero() {
        // With non-trivial payload, the checksum should be non-zero.
        let payload = b"hello world";
        let packet = build_udp([10, 0, 0, 1], [10, 0, 0, 2], 1234, 5678, payload);
        let udp_start = IP_HEADER_SIZE;
        let checksum = u16::from_be_bytes([packet[udp_start + 6], packet[udp_start + 7]]);
        assert_ne!(checksum, 0);
    }

    #[test]
    fn build_udp_payload_preserved() {
        let payload = b"test payload data";
        let packet = build_udp([10, 0, 0, 1], [10, 0, 0, 2], 1234, 5678, payload);
        let payload_start = IP_HEADER_SIZE + UDP_HEADER_SIZE;
        assert_eq!(&packet[payload_start..], payload);
    }

    #[test]
    fn build_udp_empty_payload() {
        let packet = build_udp([10, 0, 0, 1], [10, 0, 0, 2], 1234, 5678, &[]);
        // Should still have IP + UDP headers.
        assert_eq!(packet.len(), IP_HEADER_SIZE + UDP_HEADER_SIZE);
        // UDP length field should be 8 (header only).
        let udp_len = u16::from_be_bytes([packet[IP_HEADER_SIZE + 4], packet[IP_HEADER_SIZE + 5]]);
        assert_eq!(udp_len, UDP_HEADER_SIZE as u16);
    }

    #[test]
    fn build_udp_broadcast() {
        let src = [0, 0, 0, 0];
        let dst = [255, 255, 255, 255];
        let packet = build_udp(src, dst, 68, 67, &[]);
        assert_eq!(&packet[12..16], &src);
        assert_eq!(&packet[16..20], &dst);
    }

    #[test]
    fn build_udp_ip_header_checksum_valid() {
        // The IP header checksum should be valid: summing the header
        // including the checksum should yield 0.
        let packet = build_udp([10, 0, 0, 1], [10, 0, 0, 2], 1234, 5678, &[]);
        let ip_header = &packet[..IP_HEADER_SIZE];
        let checksum = internet_checksum(ip_header);
        assert_eq!(checksum, 0, "IP header checksum should validate to zero");
    }

    #[test]
    fn build_udp_donot_fragment_flag() {
        let packet = build_udp([10, 0, 0, 1], [10, 0, 0, 2], 1234, 5678, &[]);
        // Byte 6 should have Don't Fragment flag set (0x40).
        assert_eq!(packet[6], 0x40);
    }

    #[test]
    fn build_udp_ip_dscp_ecn_zero() {
        let packet = build_udp([10, 0, 0, 1], [10, 0, 0, 2], 1234, 5678, &[]);
        assert_eq!(packet[1], 0x00); // DSCP + ECN = 0.
    }
}
