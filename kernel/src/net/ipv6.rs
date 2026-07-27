//! IPv6 protocol support.
#![allow(missing_docs)]
//! IPv6 protocol handling (RFC 8200).
//!
//! Provides IPv6 header parsing, address type utilities, pseudo-header
//! construction for TCP/UDP checksums, and dispatch of incoming IPv6
//! packets to the appropriate next-header handler.
//!
//! ## IPv6 Header Format (RFC 8200, Section 3)
//!
//! ```text
//!  0                   1                   2                   3
//!  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |Version| Traffic Class |           Flow Label                  |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |         Payload Length          |  Next Header  |   Hop Limit |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                                                               |
//! +                                                               +
//! |                                                               |
//! +                         Source Address                        +
//! |                                                               |
//! +                                                               +
//! |                                                               |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                                                               |
//! +                                                               +
//! |                                                               |
//! +                      Destination Address                      +
//! |                                                               |
//! +                                                               +
//! |                                                               |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! ```
//!
//! ## IPv6 Pseudo-Header for TCP/UDP Checksum (RFC 8200, Section 8.1)
//!
//! ```text
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                                                               |
//! +                         Source Address                        +
//! |                                                               |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                                                               |
//! +                      Destination Address                      +
//! |                                                               |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                   Upper-Layer Packet Length                   |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                      zero                     |  Next Header  |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! ```
//!
//! The pseudo-header is 40 bytes: 16 (src) + 16 (dst) + 4 (length) + 4 (zeros + next header).

use alloc::vec::Vec;

use crate::drivers::net;
use crate::serial_println;

// ─────────────────── Constants ───────────────────

/// IPv6 header size (40 bytes, fixed — no IHL/options field like IPv4).
const IPV6_HEADER_SIZE: usize = 40;

/// Size of the IPv6 pseudo-header for TCP/UDP checksum calculation.
const IPV6_PSEUDO_HEADER_SIZE: usize = 40;

/// IPv6 version number (6) shifted into the high nibble of the first byte.
const IPV6_VERSION: u8 = 0x60;

/// Minimum MTU for IPv6 (1280 bytes, RFC 8200, Section 5).
#[allow(dead_code)]
const IPV6_MINIMUM_MTU: usize = 1280;

/// Default hop limit for outgoing IPv6 packets (64, same convention as IPv4).
///
/// Recommended by RFC 8200, Section 4 for "average" Internet paths.
const IPV6_DEFAULT_HOP_LIMIT: u8 = 64;

/// IPv6 Next Header value: TCP (6).
const IPV6_NH_TCP: u8 = 6;

/// IPv6 Next Header value: UDP (17).
const IPV6_NH_UDP: u8 = 17;

/// IPv6 Next Header value: ICMPv6 (58).
const IPV6_NH_ICMPV6: u8 = 58;

/// IPv6 Next Header value: No Next Header (59).
const IPV6_NH_NO_NEXT: u8 = 59;

/// Length of an IPv6 address in bytes (128 bits / 16 bytes).
const IPV6_ADDR_LEN: usize = 16;

/// Number of groups in a colon-hex IPv6 address representation.
const IPV6_ADDR_GROUPS: usize = 8;

/// Prefix length for link-local unicast addresses (fe80::/10).
const IPV6_PREFIX_LINK_LOCAL: u8 = 10;

/// IPv6 multicast prefix (ff00::/8).
const IPV6_PREFIX_MULTICAST: u8 = 8;

// ─────────────────── IPv6 Address ───────────────────

/// An IPv6 address (128-bit big-endian bytes).
///
/// Stored as 16 bytes in network byte order (most significant byte first).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ipv6Addr(pub [u8; 16]);

impl Ipv6Addr {
    /// The all-nodes link-local multicast address (`ff02::1`).
    pub const ALL_NODES_LINK_LOCAL: Ipv6Addr = Ipv6Addr([
        0xFF, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x01,
    ]);
    /// The all-routers link-local multicast address (`ff02::2`).
    pub const ALL_ROUTERS_LINK_LOCAL: Ipv6Addr = Ipv6Addr([
        0xFF, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x02,
    ]);
    /// The IPv6 loopback address (`::1`).
    pub const LOOPBACK: Ipv6Addr = Ipv6Addr([
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x01,
    ]);
    /// The solicited-node multicast prefix (`ff02::1:ff00:0/104`).
    ///
    /// Used by Neighbor Discovery for address resolution.
    pub const SOLICITED_NODE_PREFIX: [u8; 13] = [
        0xFF, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0xFF,
    ];
    /// The IPv6 unspecified address (`::`, all zeros).
    pub const UNSPECIFIED: Ipv6Addr = Ipv6Addr([0x00; 16]);

    /// Create an `Ipv6Addr` from 16 network-order bytes.
    #[must_use]
    pub const fn new(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Return `true` if this is the loopback address (`::1`).
    #[must_use]
    pub fn is_loopback(&self) -> bool {
        *self == Self::LOOPBACK
    }

    /// Return `true` if this is the unspecified address (`::`).
    #[must_use]
    pub fn is_unspecified(&self) -> bool {
        *self == Self::UNSPECIFIED
    }

    /// Return `true` if this is a link-local unicast address (fe80::/10).
    ///
    /// Link-local addresses start with `1111 1110 10` (first 10 bits).
    #[must_use]
    pub fn is_link_local(&self) -> bool {
        self.0[0] == 0xFE && (self.0[1] & 0xC0) == 0x80
    }

    /// Return `true` if this is a multicast address (ff00::/8).
    #[must_use]
    pub fn is_multicast(&self) -> bool {
        self.0[0] == 0xFF
    }

    /// Return `true` if this is a solicited-node multicast address.
    ///
    /// Format: `ff02::1:ffXX:XXXX` where XX:XXXX is the lower 24 bits of
    /// the corresponding unicast address.
    #[must_use]
    pub fn is_solicited_node_multicast(&self) -> bool {
        self.0[0..13] == Self::SOLICITED_NODE_PREFIX
    }

    /// Compute the solicited-node multicast address for this unicast address.
    ///
    /// The solicited-node address is `ff02::1:ffXX:XXXX` where `XX:XXXX`
    /// is the lower 24 bits of the unicast address.
    #[must_use]
    pub fn solicited_node_multicast(&self) -> Self {
        let mut addr = Self::UNSPECIFIED.0;
        addr[0..13].copy_from_slice(&Self::SOLICITED_NODE_PREFIX);
        // Lower 24 bits: copy bytes 13, 14, 15 of the given address.
        addr[13] = self.0[13];
        addr[14] = self.0[14];
        addr[15] = self.0[15];
        Self(addr)
    }

    /// Derive a link-local IPv6 address from a MAC address (EUI-64, RFC 4291).
    ///
    /// The algorithm inserts `0xFFFE` in the middle of the MAC and flips
    /// the U/L bit (bit 1 of byte 0).
    ///
    /// `fe80::xxxx:xxFF:FExx:xxxx`
    #[must_use]
    pub fn from_mac_eui64(mac: &[u8; 6]) -> Self {
        let mut addr = [0u8; 16];
        addr[0] = 0xFE;
        addr[1] = 0x80;
        // First half: bytes 0-2 of MAC with U/L bit flipped.
        addr[8] = mac[0] ^ 0x02; // flip U/L bit
        addr[9] = mac[1];
        addr[10] = mac[2];
        addr[11] = 0xFF;
        addr[12] = 0xFE;
        // Second half: bytes 3-5 of MAC.
        addr[13] = mac[3];
        addr[14] = mac[4];
        addr[15] = mac[5];
        Self(addr)
    }
}

impl core::fmt::Display for Ipv6Addr {
    /// Format the IPv6 address in standard colon-hex notation (RFC 5952).
    ///
    /// Groups are separated by `:`, and the longest run of zero groups is
    /// collapsed to `::` (or the first such run of length 2+ if there are
    /// multiple runs of equal length).
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let groups: [u16; IPV6_ADDR_GROUPS] = {
            let mut g = [0u16; IPV6_ADDR_GROUPS];
            for i in 0..IPV6_ADDR_GROUPS {
                g[i] = u16::from_be_bytes([self.0[i * 2], self.0[i * 2 + 1]]);
            }
            g
        };

        // Find the longest run of zero groups for :: compression.
        // If multiple runs have the same length, the first one wins (RFC 5952, Section 4.2.2).
        let mut best_start = IPV6_ADDR_GROUPS; // sentinel: no compression
        let mut best_len = 1; // only compress runs of length >= 2
        let mut cur_start = IPV6_ADDR_GROUPS;
        let mut cur_len = 0;

        for i in 0..IPV6_ADDR_GROUPS {
            if groups[i] == 0 {
                if cur_len == 0 {
                    cur_start = i;
                }
                cur_len += 1;
            } else {
                if cur_len > best_len {
                    best_start = cur_start;
                    best_len = cur_len;
                }
                cur_len = 0;
            }
        }
        // Check if a run ended at the last group.
        if cur_len > best_len {
            best_start = cur_start;
            best_len = cur_len;
        }

        // Render the address.
        let mut first = true;
        let mut i = 0;
        while i < IPV6_ADDR_GROUPS {
            if i == best_start && best_len >= 2 {
                // When the compressed region starts at the beginning, use "::"
                // (no separator needed before it). Otherwise use ":" as the
                // separator before the compressed region.
                if first {
                    f.write_str("::")?;
                    // Skip the entire compressed region.
                    i += best_len;
                    first = false;
                    // If there's nothing after the compression, we're done.
                    if i >= IPV6_ADDR_GROUPS {
                        break;
                    }
                    // Output the next group immediately with no separator,
                    // since the "::" already acts as one.
                    write!(f, "{:x}", groups[i])?;
                    i += 1;
                    continue;
                }
                // Mid-address compression: write ':' before skipping,
                // then the next non-compressed group will get another ':'
                // from the standard separator logic, yielding "::".
                f.write_str(":")?;
                i += best_len;
                continue;
            }
            if !first {
                f.write_str(":")?;
            }
            write!(f, "{:x}", groups[i])?;
            first = false;
            i += 1;
        }
        Ok(())
    }
}

// ─────────────────── IPv6 Header ───────────────────

/// Parsed IPv6 header fields (RFC 8200, Section 3).
///
/// The IPv6 header is a fixed 40-byte structure with no IHL/options field.
#[derive(Debug, Clone, Copy)]
pub struct Ipv6Header {
    /// Payload length (excludes the 40-byte header).
    payload_length: u16,
    /// Next Header value (protocol of the first extension header or upper-layer protocol).
    next_header: u8,
    /// Hop limit (replaces IPv4 TTL).
    hop_limit: u8,
    /// Source IPv6 address (16 bytes, network byte order).
    pub src_addr: Ipv6Addr,
    /// Destination IPv6 address (16 bytes, network byte order).
    pub dst_addr: Ipv6Addr,
}

/// Parse an IPv6 header from `data`.
///
/// Returns `None` if the data is too short or the version nibble is wrong.
#[must_use]
fn parse_ipv6(data: &[u8]) -> Option<Ipv6Header> {
    if data.len() < IPV6_HEADER_SIZE {
        return None;
    }

    let version_tc_flow = data[0];
    let version = version_tc_flow >> 4;

    // Only handle IPv6 (version 6 in the high nibble).
    if version != 6 {
        return None;
    }

    let payload_length = u16::from_be_bytes([data[4], data[5]]);
    let next_header = data[6];
    let hop_limit = data[7];

    let mut src_addr = [0u8; 16];
    src_addr.copy_from_slice(&data[8..24]);
    let mut dst_addr = [0u8; 16];
    dst_addr.copy_from_slice(&data[24..40]);

    Some(Ipv6Header {
        payload_length,
        next_header,
        hop_limit,
        src_addr: Ipv6Addr(src_addr),
        dst_addr: Ipv6Addr(dst_addr),
    })
}

// ─────────────────── IPv6 Pseudo-Header Checksum ───────────────────

/// Compute the IPv6 pseudo-header checksum for TCP/UDP (RFC 8200, Section 8.1).
///
/// The IPv6 pseudo-header is 40 bytes:
/// - Source address (16 bytes)
/// - Destination address (16 bytes)
/// - Upper-Layer Packet Length (4 bytes)
/// - Zero padding (3 bytes) + Next Header (1 byte)
///
/// The actual upper-layer segment (TCP or UDP header + payload) is appended
/// to the pseudo-header before computing the Internet checksum.
///
/// # Arguments
///
/// * `src_addr` - Source IPv6 address (16 bytes).
/// * `dst_addr` - Destination IPv6 address (16 bytes).
/// * `next_header` - Next Header value (e.g., 6 for TCP, 17 for UDP).
/// * `segment` - The upper-layer segment (header + payload).
///
/// # Returns
///
/// The 16-bit one's complement checksum value.
#[must_use]
pub fn ipv6_pseudo_header_checksum(
    src_addr: &Ipv6Addr,
    dst_addr: &Ipv6Addr,
    next_header: u8,
    segment: &[u8],
) -> u16 {
    let segment_len = segment.len() as u32;

    // Build pseudo-header (40 bytes) + segment.
    let mut buf = Vec::with_capacity(IPV6_PSEUDO_HEADER_SIZE + segment.len());
    buf.extend_from_slice(&src_addr.0);
    buf.extend_from_slice(&dst_addr.0);
    buf.extend_from_slice(&segment_len.to_be_bytes());
    buf.push(0); // zero padding
    buf.push(0);
    buf.push(0);
    buf.push(next_header);
    buf.extend_from_slice(segment);

    crate::net::tcp::internet_checksum(&buf)
}

// ─────────────────── ICMPv6 ───────────────────

/// ICMPv6 type: Neighbor Solicitation (135).
const ICMPV6_NEIGHBOR_SOLICITATION: u8 = 135;

/// ICMPv6 type: Neighbor Advertisement (136).
const ICMPV6_NEIGHBOR_ADVERTISEMENT: u8 = 136;

/// ICMPv6 type: Router Solicitation (133).
const ICMPV6_ROUTER_SOLICITATION: u8 = 133;

/// ICMPv6 type: Router Advertisement (134).
const ICMPV6_ROUTER_ADVERTISEMENT: u8 = 134;

/// ICMPv6 type: Echo Request (128).
const ICMPV6_ECHO_REQUEST: u8 = 128;

/// ICMPv6 type: Echo Reply (129).
const ICMPV6_ECHO_REPLY: u8 = 129;

/// Handle an incoming ICMPv6 message (RFC 4443, RFC 4861).
///
/// Currently only logs and drops. This is a stub for future ND and
/// ICMPv6 echo support.
fn handle_icmpv6(eth_src: [u8; 6], _src_addr: &Ipv6Addr, _dst_addr: &Ipv6Addr, data: &[u8]) {
    if data.is_empty() {
        serial_println!("[IPv6] ICMPv6: empty payload, dropping");
        return;
    }

    let icmpv6_type = data[0];
    let code = data.get(1).copied().unwrap_or(0);

    match icmpv6_type {
        ICMPV6_NEIGHBOR_SOLICITATION => {
            serial_println!(
                "[IPv6] ICMPv6 Neighbor Solicitation (type={}, code={}, {} bytes), dropping",
                icmpv6_type,
                code,
                data.len()
            );
            // Future: respond with Neighbor Advertisement if the target
            // address matches a local address.
        }
        ICMPV6_NEIGHBOR_ADVERTISEMENT => {
            serial_println!(
                "[IPv6] ICMPv6 Neighbor Advertisement (type={}, code={}, {} bytes), dropping",
                icmpv6_type,
                code,
                data.len()
            );
            // Future: update the Neighbor Cache with the sender's MAC.
        }
        ICMPV6_ROUTER_SOLICITATION => {
            serial_println!(
                "[IPv6] ICMPv6 Router Solicitation (type={}, code={}, {} bytes), dropping",
                icmpv6_type,
                code,
                data.len()
            );
            // Future: respond with Router Advertisement if configured
            // as a router.
        }
        ICMPV6_ROUTER_ADVERTISEMENT => {
            serial_println!(
                "[IPv6] ICMPv6 Router Advertisement (type={}, code={}, {} bytes), dropping",
                icmpv6_type,
                code,
                data.len()
            );
            // Future: learn prefix information and default route.
        }
        ICMPV6_ECHO_REQUEST => {
            serial_println!(
                "[IPv6] ICMPv6 Echo Request (type={}, code={}, {} bytes), dropping",
                icmpv6_type,
                code,
                data.len()
            );
            // Future: reply with ICMPv6 Echo Reply.
        }
        ICMPV6_ECHO_REPLY => {
            serial_println!(
                "[IPv6] ICMPv6 Echo Reply (type={}, code={}, {} bytes), dropping",
                icmpv6_type,
                code,
                data.len()
            );
        }
        _ => {
            serial_println!(
                "[IPv6] ICMPv6 unknown type {} (code={}, {} bytes), dropping",
                icmpv6_type,
                code,
                data.len()
            );
        }
    }
}

// ─────────────────── IPv6 Packet Dispatch ───────────────────

/// Dispatch an IPv6 payload to the appropriate next-header handler.
///
/// This is the entry point for incoming IPv6 packets from the Ethernet
/// frame dispatcher (`handle_frame` in `mod.rs`).
///
/// # Arguments
///
/// * `eth_src` - Source MAC address from the Ethernet header.
/// * `ipv6` - Parsed IPv6 header.
/// * `payload` - The IPv6 payload (everything after the 40-byte header).
pub fn dispatch_ipv6_payload(eth_src: [u8; 6], ipv6: &Ipv6Header, payload: &[u8]) {
    match ipv6.next_header {
        IPV6_NH_TCP => {
            serial_println!(
                "[IPv6] TCP over IPv6 from {:?}, {} bytes payload — dispatching",
                ipv6.src_addr,
                payload.len()
            );
            // Future: call tcp::handle_tcp_packet_v6(ipv6.src_addr, ipv6.dst_addr, payload)
            // when TCP supports IPv6. For now, decode and log.
            let _ = eth_src;
        }
        IPV6_NH_UDP => {
            serial_println!(
                "[IPv6] UDP over IPv6 from {:?}, {} bytes payload — dispatching",
                ipv6.src_addr,
                payload.len()
            );
            // Future: call udp::handle_incoming_udp_v6(ipv6.src_addr, payload)
            // when UDP supports IPv6. For now, log and drop.
            let _ = eth_src;
        }
        IPV6_NH_ICMPV6 => {
            handle_icmpv6(eth_src, &ipv6.src_addr, &ipv6.dst_addr, payload);
        }
        IPV6_NH_NO_NEXT => {
            serial_println!("[IPv6] No Next Header (59), dropping");
        }
        _ => {
            serial_println!(
                "[IPv6] Unknown next header {} from {:?}, dropping",
                ipv6.next_header,
                ipv6.src_addr
            );
        }
    }
}

/// Handle an IPv6 frame dispatched from the Ethernet layer.
///
/// Called from `handle_frame` in `mod.rs` when the EtherType is 0x86DD.
/// Parses the IPv6 header and dispatches the payload.
///
/// # Arguments
///
/// * `eth_src` - Source MAC address from the Ethernet header.
/// * `payload` - Raw data after the Ethernet header (should start with IPv6 header).
pub fn handle_ipv6_frame(eth_src: [u8; 6], payload: &[u8]) {
    let Some(ipv6) = parse_ipv6(payload) else {
        serial_println!("[IPv6] Header parse failed, dropping");
        return;
    };

    serial_println!(
        "[IPv6] Packet from {:?} to {:?}, next_header={}, payload_length={}",
        ipv6.src_addr,
        ipv6.dst_addr,
        ipv6.next_header,
        ipv6.payload_length
    );

    let ipv6_payload = &payload[IPV6_HEADER_SIZE..];
    dispatch_ipv6_payload(eth_src, &ipv6, ipv6_payload);
}

// ─────────────────── Formatting Helpers ───────────────────

/// Wrapper around `Ipv6Addr` for use in `serial_println!` with `{:?}`.
#[derive(Clone, Copy)]
pub struct FormatIpv6(pub Ipv6Addr);

impl core::fmt::Debug for FormatIpv6 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ─────────────────── Tests ───────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ──── Ipv6Addr tests ────

    #[test]
    fn test_loopback_identification() {
        assert!(Ipv6Addr::LOOPBACK.is_loopback());
        assert!(!Ipv6Addr::UNSPECIFIED.is_loopback());
        assert!(!Ipv6Addr::ALL_NODES_LINK_LOCAL.is_loopback());
    }

    #[test]
    fn test_unspecified_identification() {
        assert!(Ipv6Addr::UNSPECIFIED.is_unspecified());
        assert!(!Ipv6Addr::LOOPBACK.is_unspecified());
    }

    #[test]
    fn test_link_local_identification() {
        let link_local = Ipv6Addr::new([
            0xFE, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x01,
        ]);
        assert!(link_local.is_link_local());

        // fec0:: is site-local (old deprecated prefix) — should NOT match.
        let site_local = Ipv6Addr::new([
            0xFE, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x01,
        ]);
        assert!(!site_local.is_link_local());

        // Global unicast (2001::) — should not be link-local.
        let global = Ipv6Addr::new([
            0x20, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x01,
        ]);
        assert!(!global.is_link_local());

        // ::1 (loopback) — should not be link-local.
        assert!(!Ipv6Addr::LOOPBACK.is_link_local());
    }

    #[test]
    fn test_multicast_identification() {
        assert!(Ipv6Addr::ALL_NODES_LINK_LOCAL.is_multicast());
        assert!(Ipv6Addr::ALL_ROUTERS_LINK_LOCAL.is_multicast());
        assert!(!Ipv6Addr::LOOPBACK.is_multicast());
        assert!(!Ipv6Addr::UNSPECIFIED.is_multicast());
    }

    #[test]
    fn test_solicited_node_multicast_identification() {
        let sn = Ipv6Addr::new([
            0xFF, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0xFF, 0xAA,
            0xBB, 0xCC,
        ]);
        assert!(sn.is_solicited_node_multicast());

        // A non-solicited-node multicast should fail.
        let non_sn = Ipv6Addr::new([
            0xFF, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00,
            0x00, 0x01,
        ]);
        assert!(!non_sn.is_solicited_node_multicast());
    }

    #[test]
    fn test_solicited_node_multicast_computation() {
        let addr = Ipv6Addr::new([
            0xFE, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0xAA, 0xBB, 0xFF, 0xFE, 0xCC,
            0xDD, 0xEE,
        ]);
        let sn = addr.solicited_node_multicast();
        // Should be ff02::1:ffCC:DDEE
        assert_eq!(sn.0[0], 0xFF);
        assert_eq!(sn.0[1], 0x02);
        assert_eq!(sn.0[11], 0x01);
        assert_eq!(sn.0[12], 0xFF);
        assert_eq!(sn.0[13], 0xCC);
        assert_eq!(sn.0[14], 0xDD);
        assert_eq!(sn.0[15], 0xEE);
    }

    #[test]
    fn test_from_mac_eui64() {
        // MAC: 52:54:00:12:34:56 (QEMU default)
        let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let addr = Ipv6Addr::from_mac_eui64(&mac);
        // fe80::5054:ff:fe12:3456
        // (0x52 ^ 0x02 = 0x50, so first byte of interface id is 0x50)
        assert_eq!(addr.0[0], 0xFE);
        assert_eq!(addr.0[1], 0x80);
        assert_eq!(addr.0[8], 0x50); // 0x52 with U/L bit flipped
        assert_eq!(addr.0[9], 0x54);
        assert_eq!(addr.0[10], 0x00);
        assert_eq!(addr.0[11], 0xFF);
        assert_eq!(addr.0[12], 0xFE);
        assert_eq!(addr.0[13], 0x12);
        assert_eq!(addr.0[14], 0x34);
        assert_eq!(addr.0[15], 0x56);
        assert!(addr.is_link_local());
    }

    #[test]
    fn test_ipv6_display_loopback() {
        let s = alloc::format!("{}", Ipv6Addr::LOOPBACK);
        assert_eq!(s, "::1");
    }

    #[test]
    fn test_ipv6_display_unspecified() {
        let s = alloc::format!("{}", Ipv6Addr::UNSPECIFIED);
        assert_eq!(s, "::");
    }

    #[test]
    fn test_ipv6_display_link_local() {
        let addr = Ipv6Addr::new([
            0xFE, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0xAA, 0xBB, 0xFF, 0xFE, 0xCC,
            0xDD, 0xEE,
        ]);
        let s = alloc::format!("{}", addr);
        // Should be fe80::2aa:bbff:fecc:ddee (collapsing the zero block).
        assert_eq!(s, "fe80::2aa:bbff:fecc:ddee");
    }

    #[test]
    fn test_ipv6_display_global() {
        // 2001:db8::1
        let addr = Ipv6Addr::new([
            0x20, 0x01, 0x0D, 0xB8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x01,
        ]);
        let s = alloc::format!("{}", addr);
        assert_eq!(s, "2001:db8::1");
    }

    #[test]
    fn test_ipv6_display_multiple_zero_runs() {
        // 2001:db8:0:0:1:0:0:1 — first run (length 2) wins over second (length 2 also)
        let addr = Ipv6Addr::new([
            0x20, 0x01, 0x0D, 0xB8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x01,
        ]);
        let s = alloc::format!("{}", addr);
        // The first zero run at groups[2..4] has length 2.
        // The second zero run at groups[5..7] has length 2.
        // First run wins: 2001:db8:0:0:1::1? Actually groups[2..4] is length 2 => 2001:db8::1:0:0:1
        // Wait - groups[2]=0, groups[3]=0 => 2 zeros, then groups[4]=1.
        // Then groups[5]=0, groups[6]=0 => 2 zeros, then groups[7]=1.
        // Both length 2, first wins: 2001:db8::1:0:0:1
        assert_eq!(s, "2001:db8::1:0:0:1");
    }

    #[test]
    fn test_ipv6_display_no_compression() {
        // 2001:db8:1:2:3:4:5:6 — no zeros, no :: compression.
        let addr = Ipv6Addr::new([
            0x20, 0x01, 0x0D, 0xB8, 0x00, 0x01, 0x00, 0x02, 0x00, 0x03, 0x00, 0x04, 0x00, 0x05,
            0x00, 0x06,
        ]);
        let s = alloc::format!("{}", addr);
        assert_eq!(s, "2001:db8:1:2:3:4:5:6");
    }

    #[test]
    fn test_ipv6_display_multicast() {
        // ff02::1
        let s = alloc::format!("{}", Ipv6Addr::ALL_NODES_LINK_LOCAL);
        assert_eq!(s, "ff02::1");
    }

    // ──── IPv6 header parsing tests ────

    fn make_ipv6_packet(
        src: &Ipv6Addr,
        dst: &Ipv6Addr,
        next_header: u8,
        payload_len: u16,
    ) -> Vec<u8> {
        let mut buf = vec![0u8; IPV6_HEADER_SIZE + usize::from(payload_len)];
        buf[0] = IPV6_VERSION; // version=6, tc=0, flow_label=0
                               // bytes 1-3: traffic class + flow label = 0
        buf[4..6].copy_from_slice(&payload_len.to_be_bytes());
        buf[6] = next_header;
        buf[7] = IPV6_DEFAULT_HOP_LIMIT;
        buf[8..24].copy_from_slice(&src.0);
        buf[24..40].copy_from_slice(&dst.0);
        buf
    }

    #[test]
    fn test_parse_ipv6_valid() {
        let src = Ipv6Addr::new([
            0xFE, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0xAA, 0xBB, 0xFF, 0xFE, 0xCC,
            0xDD, 0xEE,
        ]);
        let dst = Ipv6Addr::ALL_NODES_LINK_LOCAL;
        let packet = make_ipv6_packet(&src, &dst, IPV6_NH_ICMPV6, 8);

        let header = parse_ipv6(&packet).unwrap();
        assert_eq!(header.payload_length, 8);
        assert_eq!(header.next_header, IPV6_NH_ICMPV6);
        assert_eq!(header.hop_limit, IPV6_DEFAULT_HOP_LIMIT);
        assert_eq!(header.src_addr, src);
        assert_eq!(header.dst_addr, dst);
    }

    #[test]
    fn test_parse_ipv6_too_short() {
        assert!(parse_ipv6(&[0u8; 39]).is_none());
    }

    #[test]
    fn test_parse_ipv6_wrong_version() {
        // IPv4 version (4) in high nibble — should be rejected.
        let mut data = vec![0u8; IPV6_HEADER_SIZE];
        data[0] = 0x45; // version=4
        assert!(parse_ipv6(&data).is_none());
    }

    #[test]
    fn test_parse_ipv6_with_payload() {
        let src = Ipv6Addr::LOOPBACK;
        let dst = Ipv6Addr::LOOPBACK;
        let mut packet = make_ipv6_packet(&src, &dst, IPV6_NH_TCP, 20);
        // Add some payload bytes.
        packet.extend_from_slice(&[0x01, 0x02, 0x03, 0x04]);

        let header = parse_ipv6(&packet).unwrap();
        assert_eq!(header.payload_length, 20); // original payload length
        assert_eq!(header.next_header, IPV6_NH_TCP);
    }

    // ──── IPv6 pseudo-header checksum tests ────

    #[test]
    fn test_ipv6_pseudo_checksum_deterministic() {
        let src = Ipv6Addr::new([
            0xFE, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0xAA, 0xBB, 0xFF, 0xFE, 0xCC,
            0xDD, 0xEE,
        ]);
        let dst = Ipv6Addr::ALL_NODES_LINK_LOCAL;
        let segment = [0x00, 0x35, 0x00, 0x01, 0x00, 0x08, 0x00, 0x00];
        let c1 = ipv6_pseudo_header_checksum(&src, &dst, IPV6_NH_UDP, &segment);
        let c2 = ipv6_pseudo_header_checksum(&src, &dst, IPV6_NH_UDP, &segment);
        assert_eq!(c1, c2, "checksum should be deterministic");
    }

    #[test]
    fn test_ipv6_pseudo_checksum_differs_with_different_addr() {
        let src1 = Ipv6Addr::LOOPBACK;
        let src2 = Ipv6Addr::UNSPECIFIED;
        let dst = Ipv6Addr::ALL_NODES_LINK_LOCAL;
        let segment = [0x00, 0x35, 0x00, 0x01, 0x00, 0x08, 0x00, 0x00];
        let c1 = ipv6_pseudo_header_checksum(&src1, &dst, IPV6_NH_UDP, &segment);
        let c2 = ipv6_pseudo_header_checksum(&src2, &dst, IPV6_NH_UDP, &segment);
        assert_ne!(
            c1, c2,
            "checksum should differ for different source addresses"
        );
    }

    #[test]
    fn test_ipv6_pseudo_checksum_empty_segment() {
        let src = Ipv6Addr::LOOPBACK;
        let dst = Ipv6Addr::LOOPBACK;
        let checksum = ipv6_pseudo_header_checksum(&src, &dst, IPV6_NH_UDP, &[]);
        // Should not panic and return a valid checksum.
        assert_ne!(
            checksum, 0,
            "empty segment checksum should be non-zero unless everything is zero"
        );
    }

    #[test]
    fn test_ipv6_pseudo_checksum_differs_for_tcp_vs_udp() {
        let src = Ipv6Addr::LOOPBACK;
        let dst = Ipv6Addr::ALL_NODES_LINK_LOCAL;
        let segment = [0x01, 0x02, 0x03, 0x04];
        let c_tcp = ipv6_pseudo_header_checksum(&src, &dst, IPV6_NH_TCP, &segment);
        let c_udp = ipv6_pseudo_header_checksum(&src, &dst, IPV6_NH_UDP, &segment);
        assert_ne!(
            c_tcp, c_udp,
            "TCP and UDP checksums should differ due to different next_header"
        );
    }

    // ──── FormatIpv6 tests ────

    #[test]
    fn test_format_ipv6_debug() {
        let fmt = FormatIpv6(Ipv6Addr::LOOPBACK);
        let s = alloc::format!("{:?}", fmt);
        assert_eq!(s, "::1");
    }

    // ──── Constant validation tests ────

    #[test]
    fn test_ipv6_constants() {
        assert_eq!(IPV6_HEADER_SIZE, 40);
        assert_eq!(IPV6_PSEUDO_HEADER_SIZE, 40);
        assert_eq!(IPV6_DEFAULT_HOP_LIMIT, 64);
        assert_eq!(IPV6_NH_TCP, 6);
        assert_eq!(IPV6_NH_UDP, 17);
        assert_eq!(IPV6_NH_ICMPV6, 58);
        assert_eq!(IPV6_ADDR_LEN, 16);
    }

    #[test]
    fn test_icmpv6_types() {
        assert_eq!(ICMPV6_NEIGHBOR_SOLICITATION, 135);
        assert_eq!(ICMPV6_NEIGHBOR_ADVERTISEMENT, 136);
        assert_eq!(ICMPV6_ECHO_REQUEST, 128);
        assert_eq!(ICMPV6_ECHO_REPLY, 129);
    }
}
