//! UDP (User Datagram Protocol) packet construction.
//!
//! Provides minimal UDP header construction for building outgoing UDP
//! datagrams. Used by the DHCP client to encapsulate DHCP messages.
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

use alloc::vec;
use alloc::vec::Vec;

/// UDP header size in bytes.
const UDP_HEADER_SIZE: usize = 8;

/// DHCP server port (well-known, RFC 2131).
pub const DHCP_SERVER_PORT: u16 = 67;

/// DHCP client port (well-known, RFC 2131).
pub const DHCP_CLIENT_PORT: u16 = 68;

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
