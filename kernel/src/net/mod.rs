//! Network protocol stack.
//!
//! Provides ARP and ICMP ping handling on top of the raw Ethernet frame
//! interface exposed by `drivers::net`. Runs in kernel space as a service
//! loop that dispatches incoming frames to the appropriate protocol handler.
//!
//! ## Architecture
//!
//! ```text
//! virtio-net driver (drivers::net)
//!     |
//!     | raw Ethernet frames
//!     v
//! net::handle_frame()  <-- parses Ethernet header
//!     |
//!     +-- ARP: handle_arp()  --> ARP table lookup / respond
//!     +-- ICMP: handle_icmp() --> echo reply
//!     +-- UDP: handle_udp()  --> DHCP client
//!     +-- other: dropped (logged)
//! ```
//!
//! ## Constants
//!
//! All numeric values are documented named constants — no magic numbers.

pub mod dhcp;
pub mod dns;
pub mod socket;
pub mod tcp;
pub mod udp;

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

use spin::Mutex;

use crate::drivers::net;
use crate::serial_println;

// ─────────────────── Ethernet constants ───────────────────

/// Ethernet header size: 6 dst + 6 src + 2 ethertype.
const ETHERNET_HEADER_SIZE: usize = 14;

/// Minimum Ethernet frame size (header only).
const ETHERNET_MIN_FRAME: usize = 64;

/// `EtherType` for ARP (0x0806).
const ETHERTYPE_ARP: u16 = 0x0806;

/// `EtherType` for IPv4 (0x0800).
const ETHERTYPE_IPV4: u16 = 0x0800;

/// Broadcast MAC address (ff:ff:ff:ff:ff:ff).
const BROADCAST_MAC: [u8; 6] = [0xFF; 6];

/// Zero MAC address (used in ARP probes).
const ZERO_MAC: [u8; 6] = [0x00; 6];

// ─────────────────── ARP constants ───────────────────

/// ARP hardware type: Ethernet (1).
const ARP_HW_TYPE_ETHERNET: u16 = 1;

/// ARP protocol type: IPv4 (0x0800).
const ARP_PROTO_TYPE_IPV4: u16 = 0x0800;

/// ARP hardware address length: 6 bytes (MAC).
const ARP_HW_LEN: u8 = 6;

/// ARP protocol address length: 4 bytes (IPv4).
const ARP_PROTO_LEN: u8 = 4;

/// ARP opcode: request (1).
const ARP_OP_REQUEST: u16 = 1;

/// ARP opcode: reply (2).
const ARP_OP_REPLY: u16 = 2;

/// ARP header size: 8 bytes fixed fields + 20 bytes addresses.
const ARP_HEADER_SIZE: usize = 28;

/// ARP entry expiry time in ticks (60 seconds at ~100 Hz timer frequency).
const ARP_EXPIRY_TICKS: u64 = 6000;

// ─────────────────── ICMP constants ───────────────────

/// ICMP type: echo request.
const ICMP_TYPE_ECHO_REQUEST: u8 = 8;

/// ICMP type: echo reply.
const ICMP_TYPE_ECHO_REPLY: u8 = 0;

/// ICMP code: 0 (standard).
const ICMP_CODE_ZERO: u8 = 0;

/// ICMP header size: type(1) + code(1) + checksum(2) + id(2) + seq(2).
const ICMP_HEADER_SIZE: usize = 8;

// ─────────────────── Protocol number constants ───────────────────

/// IPv4 protocol number for ICMP (1).
const IP_PROTO_ICMP: u8 = 1;

/// IPv4 protocol number for TCP (6).
const IP_PROTO_TCP: u8 = 6;

/// IPv4 protocol number for UDP (17).
const IP_PROTO_UDP: u8 = 17;

/// IPv4 header minimum size (20 bytes, no options).
const IP_HEADER_MIN_SIZE: usize = 20;

/// Default IPv4 Time-To-Live (hop limit).
const IP_DEFAULT_TTL: u8 = 64;

/// IPv4 version (4) shifted into the high nibble.
const IP_VERSION_4: u8 = 4;

/// IPv4 header length in 32-bit words (5 = 20 bytes, no options).
const IP_IHL_NO_OPTIONS: u8 = 5;

// ─────────────────── Structures ───────────────────

/// Parsed Ethernet frame header.
#[derive(Debug, Clone, Copy)]
struct EthernetHeader {
    /// Destination MAC address.
    dst_mac: [u8; 6],
    /// Source MAC address.
    src_mac: [u8; 6],
    /// `EtherType` field (protocol identifier).
    ethertype: u16,
}

/// Parsed ARP header (IPv4 over Ethernet).
#[derive(Debug, Clone, Copy)]
struct ArpHeader {
    /// Hardware type (1 = Ethernet).
    hw_type: u16,
    /// Protocol type (0x0800 = IPv4).
    proto_type: u16,
    /// Hardware address length (6 for MAC).
    hw_len: u8,
    /// Protocol address length (4 for IPv4).
    proto_len: u8,
    /// Operation: 1 = request, 2 = reply.
    opcode: u16,
    /// Sender MAC address.
    sender_mac: [u8; 6],
    /// Sender IPv4 address (network byte order).
    sender_ip: u32,
    /// Target MAC address.
    target_mac: [u8; 6],
    /// Target IPv4 address (network byte order).
    target_ip: u32,
}

/// Parsed ICMP echo header.
#[derive(Debug, Clone, Copy)]
struct IcmpHeader {
    /// ICMP message type.
    msg_type: u8,
    /// ICMP code.
    code: u8,
    /// Header checksum.
    checksum: u16,
    /// Identifier (echo id).
    id: u16,
    /// Sequence number.
    seq: u16,
}

/// Parsed IPv4 header (minimal fields for ICMP dispatch).
#[derive(Debug, Clone, Copy)]
struct Ipv4Header {
    /// IP protocol number (e.g. 1 = ICMP).
    protocol: u8,
    /// Source IPv4 address.
    src_ip: u32,
    /// Destination IPv4 address.
    dst_ip: u32,
    /// Total IPv4 packet length.
    total_len: u16,
    /// IPv4 header length in bytes (including options).
    header_len: usize,
}

/// ARP table entry: cached MAC address with timestamp for expiration.
#[derive(Debug, Clone, Copy)]
struct ArpEntry {
    /// MAC address of the remote host.
    mac: [u8; 6],
    /// Tick count when this entry was added or last updated.
    timestamp: u64,
}

/// ARP table: maps IPv4 address (network byte order) to `ArpEntry`.
static ARP_TABLE: Mutex<BTreeMap<u32, ArpEntry>> = Mutex::new(BTreeMap::new());

/// Fallback local IP address (10.0.2.15 in network byte order).
///
/// Used when DHCP has not yet assigned an address. QEMU's default DHCP
/// server assigns this address, so it is a safe fallback for development.
const DEFAULT_LOCAL_IP: u32 = 0x0F_02_00_0A; // 10.0.2.15 in big-endian

/// Get the local IP address, preferring the DHCP-assigned address.
///
/// If DHCP has completed, returns the assigned address. Otherwise falls
/// back to `DEFAULT_LOCAL_IP` (10.0.2.15).
fn local_ip() -> u32 {
    let state = dhcp::get_network_state();
    if state.configured {
        u32::from_be_bytes(state.ip)
    } else {
        DEFAULT_LOCAL_IP
    }
}

// ─────────────────── Byte helpers ───────────────────

/// Read a big-endian `u16` from `data[offset..]`.
///
/// # Panics
/// Panics if `offset + 2 > data.len()`.
fn read_u16_be(data: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([data[offset], data[offset + 1]])
}

/// Read a big-endian `u32` from `data[offset..]`.
///
/// # Panics
/// Panics if `offset + 4 > data.len()`.
fn read_u32_be(data: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

/// Write a big-endian `u16` into `buf[offset..]`.
fn write_u16_be(buf: &mut [u8], offset: usize, value: u16) {
    let bytes = value.to_be_bytes();
    buf[offset] = bytes[0];
    buf[offset + 1] = bytes[1];
}

/// Write a big-endian `u32` into `buf[offset..]`.
fn write_u32_be(buf: &mut [u8], offset: usize, value: u32) {
    let bytes = value.to_be_bytes();
    buf[offset] = bytes[0];
    buf[offset + 1] = bytes[1];
    buf[offset + 2] = bytes[2];
    buf[offset + 3] = bytes[3];
}

// ─────────────────── Ethernet parsing ───────────────────

/// Parse an Ethernet frame into header and payload.
///
/// Returns `None` if the frame is too short to contain a valid header.
fn parse_ethernet(data: &[u8]) -> Option<(EthernetHeader, &[u8])> {
    if data.len() < ETHERNET_HEADER_SIZE {
        return None;
    }

    let header = EthernetHeader {
        dst_mac: [data[0], data[1], data[2], data[3], data[4], data[5]],
        src_mac: [data[6], data[7], data[8], data[9], data[10], data[11]],
        ethertype: read_u16_be(data, 12),
    };

    Some((header, &data[ETHERNET_HEADER_SIZE..]))
}

/// Build an Ethernet frame from components.
///
/// Returns a `Vec<u8>` containing the complete frame (header + payload).
fn build_ethernet(dst: [u8; 6], src: [u8; 6], ethertype: u16, payload: &[u8]) -> Vec<u8> {
    let total = ETHERNET_HEADER_SIZE + payload.len();
    let mut frame = vec![0u8; total];

    frame[0..6].copy_from_slice(&dst);
    frame[6..12].copy_from_slice(&src);
    write_u16_be(&mut frame, 12, ethertype);
    frame[ETHERNET_HEADER_SIZE..].copy_from_slice(payload);

    frame
}

// ─────────────────── ARP handling ───────────────────

/// Parse an ARP header from `data`.
///
/// Returns `None` if the data is too short or the ARP is not IPv4/Ethernet.
fn parse_arp(data: &[u8]) -> Option<ArpHeader> {
    if data.len() < ARP_HEADER_SIZE {
        return None;
    }

    let hw_type = read_u16_be(data, 0);
    let proto_type = read_u16_be(data, 2);
    let hw_len = data[4];
    let proto_len = data[5];
    let opcode = read_u16_be(data, 6);

    // Only handle Ethernet + IPv4 ARP.
    if hw_type != ARP_HW_TYPE_ETHERNET
        || proto_type != ARP_PROTO_TYPE_IPV4
        || hw_len != ARP_HW_LEN
        || proto_len != ARP_PROTO_LEN
    {
        return None;
    }

    let sender_mac = [data[8], data[9], data[10], data[11], data[12], data[13]];
    let sender_ip = read_u32_be(data, 14);
    let target_mac = [data[18], data[19], data[20], data[21], data[22], data[23]];
    let target_ip = read_u32_be(data, 24);

    Some(ArpHeader {
        hw_type,
        proto_type,
        hw_len,
        proto_len,
        opcode,
        sender_mac,
        sender_ip,
        target_mac,
        target_ip,
    })
}

/// Build an ARP reply payload.
///
/// Constructs the 28-byte ARP response for an Ethernet/IPv4 ARP request.
fn build_arp_reply(
    sender_mac: [u8; 6],
    sender_ip: u32,
    target_mac: [u8; 6],
    target_ip: u32,
) -> Vec<u8> {
    let mut reply = vec![0u8; ARP_HEADER_SIZE];

    write_u16_be(&mut reply, 0, ARP_HW_TYPE_ETHERNET);
    write_u16_be(&mut reply, 2, ARP_PROTO_TYPE_IPV4);
    reply[4] = ARP_HW_LEN;
    reply[5] = ARP_PROTO_LEN;
    write_u16_be(&mut reply, 6, ARP_OP_REPLY);

    reply[8..14].copy_from_slice(&sender_mac);
    write_u32_be(&mut reply, 14, sender_ip);
    reply[18..24].copy_from_slice(&target_mac);
    write_u32_be(&mut reply, 24, target_ip);

    reply
}

/// Build an ARP request payload for `target_ip`.
///
/// Sends a "who has `target_ip`?" broadcast.
fn build_arp_request(sender_mac: [u8; 6], sender_ip: u32, target_ip: u32) -> Vec<u8> {
    let mut request = vec![0u8; ARP_HEADER_SIZE];

    write_u16_be(&mut request, 0, ARP_HW_TYPE_ETHERNET);
    write_u16_be(&mut request, 2, ARP_PROTO_TYPE_IPV4);
    request[4] = ARP_HW_LEN;
    request[5] = ARP_PROTO_LEN;
    write_u16_be(&mut request, 6, ARP_OP_REQUEST);

    request[8..14].copy_from_slice(&sender_mac);
    write_u32_be(&mut request, 14, sender_ip);
    request[18..24].copy_from_slice(&ZERO_MAC);
    write_u32_be(&mut request, 24, target_ip);

    request
}

/// Handle an incoming ARP frame.
///
/// - ARP request for our IP: send ARP reply and update table.
/// - ARP reply: update the ARP table.
/// - Otherwise: log and drop.
fn handle_arp(eth: &EthernetHeader, payload: &[u8]) {
    let Some(arp) = parse_arp(payload) else {
        serial_println!("[NET] ARP: parse failed, dropping");
        return;
    };

    // Learn the sender's MAC regardless of opcode.
    if arp.sender_ip != 0 {
        let now =
            crate::arch::x86_64::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);
        ARP_TABLE.lock().insert(
            arp.sender_ip,
            ArpEntry {
                mac: arp.sender_mac,
                timestamp: now,
            },
        );
    }

    match arp.opcode {
        ARP_OP_REQUEST => {
            serial_println!(
                "[NET] ARP request: who has {:?} from {:?}",
                format_ip(arp.target_ip),
                format_ip(arp.sender_ip)
            );

            // Only reply if the request is for our IP.
            if arp.target_ip != local_ip() {
                return;
            }

            let local_mac = net::mac_address();
            let reply_payload =
                build_arp_reply(local_mac, local_ip(), arp.sender_mac, arp.sender_ip);
            let frame = build_ethernet(arp.sender_mac, local_mac, ETHERTYPE_ARP, &reply_payload);

            match net::send_frame(&frame) {
                Ok(sent) => {
                    serial_println!(
                        "[NET] ARP reply sent to {:?} ({:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x})",
                        format_ip(arp.sender_ip),
                        arp.sender_mac[0],
                        arp.sender_mac[1],
                        arp.sender_mac[2],
                        arp.sender_mac[3],
                        arp.sender_mac[4],
                        arp.sender_mac[5]
                    );
                }
                Err(e) => {
                    serial_println!("[NET] ARP reply send failed: {:?}", e);
                }
            }
        }
        ARP_OP_REPLY => {
            serial_println!(
                "[NET] ARP reply: {:?} is at {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                format_ip(arp.sender_ip),
                arp.sender_mac[0],
                arp.sender_mac[1],
                arp.sender_mac[2],
                arp.sender_mac[3],
                arp.sender_mac[4],
                arp.sender_mac[5]
            );
        }
        _ => {
            serial_println!("[NET] ARP: unknown opcode {}", arp.opcode);
        }
    }
}

/// Send an ARP request for `target_ip` (broadcast).
///
/// Looks up the local MAC from the network driver and sends
/// a broadcast ARP request. The response will be handled by
/// `handle_arp` when it arrives.
pub fn send_arp_request(target_ip: u32) {
    let local_mac = net::mac_address();
    let request_payload = build_arp_request(local_mac, local_ip(), target_ip);
    let frame = build_ethernet(BROADCAST_MAC, local_mac, ETHERTYPE_ARP, &request_payload);

    match net::send_frame(&frame) {
        Ok(_) => {
            serial_println!("[NET] ARP request sent for {:?}", format_ip(target_ip));
        }
        Err(e) => {
            serial_println!("[NET] ARP request failed: {:?}", e);
        }
    }
}

/// Look up a MAC address in the ARP table.
///
/// Returns `Some(mac)` if the IP is known and the entry has not expired,
/// `None` otherwise. Expired entries are removed on lookup.
pub fn arp_lookup(ip: u32) -> Option<[u8; 6]> {
    let now = crate::arch::x86_64::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);
    let mut table = ARP_TABLE.lock();
    if let Some(entry) = table.get(&ip) {
        if now.saturating_sub(entry.timestamp) < ARP_EXPIRY_TICKS {
            return Some(entry.mac);
        }
        // Entry has expired — remove it.
        table.remove(&ip);
    }
    None
}

/// Remove all ARP entries older than `ARP_EXPIRY_TICKS`.
///
/// Called periodically from the network service loop to prevent stale
/// entries from accumulating. Uses `saturating_sub` to avoid underflow
/// if the tick counter wraps (unlikely with `u64`).
pub fn expire_arp_entries() {
    let now = crate::arch::x86_64::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);
    let mut table = ARP_TABLE.lock();
    table.retain(|_ip, entry| now.saturating_sub(entry.timestamp) < ARP_EXPIRY_TICKS);
}

// ─────────────────── IPv4 parsing ───────────────────

/// Parse the IPv4 header from `data`.
///
/// Returns `None` if the data is too short or the version/IHL is invalid.
fn parse_ipv4(data: &[u8]) -> Option<Ipv4Header> {
    if data.len() < IP_HEADER_MIN_SIZE {
        return None;
    }

    let version_ihl = data[0];
    let version = version_ihl >> 4;
    let ihl = version_ihl & 0x0F;

    // Only handle IPv4 with standard 20-byte header (no options).
    if version != IP_VERSION_4 || ihl < IP_IHL_NO_OPTIONS {
        return None;
    }

    let header_len = usize::from(ihl) * 4;
    if data.len() < header_len {
        return None;
    }

    let total_len = read_u16_be(data, 2);
    let protocol = data[9];
    let src_ip = read_u32_be(data, 12);
    let dst_ip = read_u32_be(data, 16);

    Some(Ipv4Header {
        protocol,
        src_ip,
        dst_ip,
        total_len,
        header_len,
    })
}

// ─────────────────── ICMP handling ───────────────────

/// Parse an ICMP echo header from `data`.
///
/// Returns `None` if the data is too short.
fn parse_icmp(data: &[u8]) -> Option<IcmpHeader> {
    if data.len() < ICMP_HEADER_SIZE {
        return None;
    }

    Some(IcmpHeader {
        msg_type: data[0],
        code: data[1],
        checksum: read_u16_be(data, 2),
        id: read_u16_be(data, 4),
        seq: read_u16_be(data, 6),
    })
}

/// Compute the Internet checksum over `data`.
///
/// The checksum is the 16-bit one's complement of the one's complement
/// sum of all 16-bit words. If the data length is odd, the last byte
/// is zero-padded.
fn internet_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;

    // Process 16-bit words.
    let mut i = 0;
    while i + 1 < data.len() {
        sum += u32::from(read_u16_be(data, i));
        i += 2;
    }

    // Handle odd-length data by zero-padding the last byte.
    if i < data.len() {
        sum += u32::from(data[i]) << 8;
    }

    // Fold 32-bit sum into 16 bits.
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    !sum as u16
}

/// Handle an ICMP echo request by sending an echo reply.
///
/// Constructs the reply by swapping src/dst in the IPv4 header,
/// changing the ICMP type to echo reply, and recalculating checksums.
fn handle_icmp(eth_src: [u8; 6], ipv4: &Ipv4Header, icmp_payload: &[u8]) {
    let Some(icmp) = parse_icmp(icmp_payload) else {
        serial_println!("[NET] ICMP: parse failed, dropping");
        return;
    };

    if icmp.msg_type != ICMP_TYPE_ECHO_REQUEST {
        serial_println!(
            "[NET] ICMP: type {} (not echo request), dropping",
            icmp.msg_type
        );
        return;
    }

    serial_println!(
        "[NET] ICMP echo request: id={:#06x} seq={} from {:?}",
        icmp.id,
        icmp.seq,
        format_ip(ipv4.src_ip)
    );

    // Build ICMP echo reply: same payload, type changed to 0, checksum recalculated.
    let mut reply_icmp = vec![0u8; icmp_payload.len()];
    reply_icmp[0] = ICMP_TYPE_ECHO_REPLY;
    reply_icmp[1] = ICMP_CODE_ZERO;
    // Checksum field (bytes 2-3) = 0 for calculation.
    reply_icmp[2] = 0;
    reply_icmp[3] = 0;
    // Copy id, seq, and data from request.
    reply_icmp[4..].copy_from_slice(&icmp_payload[4..]);

    let checksum = internet_checksum(&reply_icmp);
    write_u16_be(&mut reply_icmp, 2, checksum);

    // Build IPv4 header for the reply (swap src/dst).
    let ip_total_len = (IP_HEADER_MIN_SIZE + reply_icmp.len()) as u16;
    let mut ip_header = vec![0u8; IP_HEADER_MIN_SIZE];
    ip_header[0] = (IP_VERSION_4 << 4) | IP_IHL_NO_OPTIONS;
    write_u16_be(&mut ip_header, 2, ip_total_len);
    ip_header[8] = IP_DEFAULT_TTL;
    ip_header[9] = IP_PROTO_ICMP;
    write_u32_be(&mut ip_header, 12, ipv4.dst_ip); // src = our IP
    write_u32_be(&mut ip_header, 16, ipv4.src_ip); // dst = requester

    let ip_checksum = internet_checksum(&ip_header);
    write_u16_be(&mut ip_header, 10, ip_checksum);

    // Combine IPv4 header + ICMP payload.
    let mut ip_packet = Vec::with_capacity(IP_HEADER_MIN_SIZE + reply_icmp.len());
    ip_packet.extend_from_slice(&ip_header);
    ip_packet.extend_from_slice(&reply_icmp);

    // Build Ethernet frame.
    let local_mac = net::mac_address();
    let frame = build_ethernet(eth_src, local_mac, ETHERTYPE_IPV4, &ip_packet);

    match net::send_frame(&frame) {
        Ok(sent) => {
            serial_println!(
                "[NET] ICMP echo reply sent to {:?} ({} bytes)",
                format_ip(ipv4.src_ip),
                sent
            );
        }
        Err(e) => {
            serial_println!("[NET] ICMP reply send failed: {:?}", e);
        }
    }
}

// ─────────────────── Frame dispatcher ───────────────────

/// Handle a single incoming Ethernet frame.
///
/// Parses the Ethernet header and dispatches to the appropriate
/// protocol handler based on the `EtherType`.
fn handle_frame(data: &[u8]) {
    let Some((eth, payload)) = parse_ethernet(data) else {
        serial_println!("[NET] Frame too short ({} bytes), dropping", data.len());
        return;
    };

    match eth.ethertype {
        ETHERTYPE_ARP => {
            handle_arp(&eth, payload);
        }
        ETHERTYPE_IPV4 => {
            let Some(ipv4) = parse_ipv4(payload) else {
                serial_println!("[NET] IPv4 parse failed, dropping");
                return;
            };

            if ipv4.protocol == IP_PROTO_ICMP {
                let icmp_data = &payload[ipv4.header_len..];
                handle_icmp(eth.src_mac, &ipv4, icmp_data);
            } else if ipv4.protocol == IP_PROTO_TCP {
                let tcp_data = &payload[ipv4.header_len..];
                tcp::handle_tcp_packet(ipv4.src_ip, ipv4.dst_ip, tcp_data);
            } else if ipv4.protocol == IP_PROTO_UDP {
                // UDP frames are handled by the DHCP client during negotiation.
                // The service loop logs them but does not process them further
                // once DHCP is complete.
                serial_println!(
                    "[NET] UDP from {:?} ({} bytes, forwarded to DHCP client)",
                    format_ip(ipv4.src_ip),
                    ipv4.total_len
                );
            } else {
                serial_println!(
                    "[NET] IPv4: protocol {} from {:?}, dropping",
                    ipv4.protocol,
                    format_ip(ipv4.src_ip)
                );
            }
        }
        other => {
            serial_println!("[NET] Unknown EtherType {:#06x}, dropping", other);
        }
    }
}

// ─────────────────── Service loop ───────────────────

/// Interval (in ticks) between ARP table expiration sweeps.
const ARP_EXPIRE_CHECK_INTERVAL: u64 = 1000;

/// Network service loop.
///
/// Continuously polls for incoming Ethernet frames and dispatches
/// them to the appropriate protocol handler. Also checks DHCP lease
/// renewal periodically (RFC 2131 T1 at 50% of lease time) and
/// expires stale ARP entries.
///
/// This function does not return — it loops forever with HLT between
/// polls to conserve CPU.
pub fn net_service_loop() -> ! {
    serial_println!("[NET] Service loop started");

    let mut last_arp_expire_tick: u64 = 0;

    loop {
        // Non-blocking poll for received frames.
        if let Some(frame) = net::receive_frame() {
            handle_frame(&frame);
        }

        // Check if the DHCP lease needs renewal (T1 = 50% of lease time).
        let mac = net::mac_address();
        dhcp::check_lease_renewal(mac, |frame| net::send_frame(frame), net::receive_frame);

        // Periodically expire stale ARP entries.
        let now =
            crate::arch::x86_64::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);
        if now.saturating_sub(last_arp_expire_tick) >= ARP_EXPIRE_CHECK_INTERVAL {
            expire_arp_entries();
            last_arp_expire_tick = now;
        }

        // HLT until next interrupt to avoid busy-spinning.
        x86_64::instructions::hlt();
    }
}

// ─────────────────── Formatting helpers ───────────────────

/// Format an IPv4 address (network byte order) as a dotted-quad string.
///
/// Returns a `FormatIp` wrapper that implements `core::fmt::Debug`
/// for use in `serial_println!`.
fn format_ip(ip: u32) -> FormatIp {
    FormatIp(ip)
}

/// Wrapper for formatting IPv4 addresses in dotted-quad notation.
struct FormatIp(u32);

impl core::fmt::Debug for FormatIp {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let b = self.0.to_be_bytes();
        write!(f, "{}.{}.{}.{}", b[0], b[1], b[2], b[3])
    }
}

// ─────────────────── Tests ───────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_write_u16_be() {
        let mut buf = [0u8; 2];
        write_u16_be(&mut buf, 0, 0x1234);
        assert_eq!(buf, [0x12, 0x34]);
        assert_eq!(read_u16_be(&buf, 0), 0x1234);
    }

    #[test]
    fn test_read_write_u32_be() {
        let mut buf = [0u8; 4];
        write_u32_be(&mut buf, 0, 0x01020304);
        assert_eq!(buf, [0x01, 0x02, 0x03, 0x04]);
        assert_eq!(read_u32_be(&buf, 0), 0x01020304);
    }

    #[test]
    fn test_parse_ethernet_valid() {
        let mut frame = vec![0u8; ETHERNET_HEADER_SIZE + 4];
        frame[0..6].copy_from_slice(&[0xAA; 6]);
        frame[6..12].copy_from_slice(&[0xBB; 6]);
        write_u16_be(&mut frame, 12, ETHERTYPE_ARP);

        let (hdr, payload) = parse_ethernet(&frame).unwrap();
        assert_eq!(hdr.dst_mac, [0xAA; 6]);
        assert_eq!(hdr.src_mac, [0xBB; 6]);
        assert_eq!(hdr.ethertype, ETHERTYPE_ARP);
        assert_eq!(payload.len(), 4);
    }

    #[test]
    fn test_parse_ethernet_too_short() {
        assert!(parse_ethernet(&[0u8; 13]).is_none());
    }

    #[test]
    fn test_build_ethernet() {
        let payload = [0x01, 0x02, 0x03];
        let frame = build_ethernet([0xAA; 6], [0xBB; 6], ETHERTYPE_IPV4, &payload);
        assert_eq!(frame.len(), ETHERNET_HEADER_SIZE + 3);
        assert_eq!(&frame[0..6], &[0xAA; 6]);
        assert_eq!(&frame[6..12], &[0xBB; 6]);
        assert_eq!(read_u16_be(&frame, 12), ETHERTYPE_IPV4);
        assert_eq!(&frame[14..], &[0x01, 0x02, 0x03]);
    }

    #[test]
    fn test_parse_arp_valid() {
        let mut data = vec![0u8; ARP_HEADER_SIZE];
        write_u16_be(&mut data, 0, ARP_HW_TYPE_ETHERNET);
        write_u16_be(&mut data, 2, ARP_PROTO_TYPE_IPV4);
        data[4] = ARP_HW_LEN;
        data[5] = ARP_PROTO_LEN;
        write_u16_be(&mut data, 6, ARP_OP_REQUEST);
        data[8..14].copy_from_slice(&[0xAA; 6]);
        write_u32_be(&mut data, 14, 0x0100A8C0); // 192.168.0.1
        data[18..24].copy_from_slice(&[0x00; 6]);
        write_u32_be(&mut data, 24, 0x0200A8C0); // 192.168.0.2

        let arp = parse_arp(&data).unwrap();
        assert_eq!(arp.hw_type, ARP_HW_TYPE_ETHERNET);
        assert_eq!(arp.opcode, ARP_OP_REQUEST);
        assert_eq!(arp.sender_mac, [0xAA; 6]);
        assert_eq!(arp.sender_ip, 0x0100A8C0);
        assert_eq!(arp.target_ip, 0x0200A8C0);
    }

    #[test]
    fn test_parse_arp_wrong_hw_type() {
        let mut data = vec![0u8; ARP_HEADER_SIZE];
        write_u16_be(&mut data, 0, 2); // Not Ethernet
        write_u16_be(&mut data, 2, ARP_PROTO_TYPE_IPV4);
        data[4] = ARP_HW_LEN;
        data[5] = ARP_PROTO_LEN;
        write_u16_be(&mut data, 6, ARP_OP_REQUEST);

        assert!(parse_arp(&data).is_none());
    }

    #[test]
    fn test_parse_arp_too_short() {
        assert!(parse_arp(&[0u8; 20]).is_none());
    }

    #[test]
    fn test_parse_icmp_valid() {
        let mut data = vec![0u8; ICMP_HEADER_SIZE];
        data[0] = ICMP_TYPE_ECHO_REQUEST;
        data[1] = ICMP_CODE_ZERO;
        write_u16_be(&mut data, 4, 0x1234); // id
        write_u16_be(&mut data, 6, 1); // seq

        let icmp = parse_icmp(&data).unwrap();
        assert_eq!(icmp.msg_type, ICMP_TYPE_ECHO_REQUEST);
        assert_eq!(icmp.id, 0x1234);
        assert_eq!(icmp.seq, 1);
    }

    #[test]
    fn test_parse_icmp_too_short() {
        assert!(parse_icmp(&[0u8; 7]).is_none());
    }

    #[test]
    fn test_internet_checksum_empty() {
        // Checksum of empty data is 0xFFFF (all ones).
        assert_eq!(internet_checksum(&[]), 0xFFFF);
    }

    #[test]
    fn test_internet_checksum_known() {
        // ICMP echo request: type=8, code=0, checksum=0, id=1, seq=1.
        let data = [0x08, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01];
        let checksum = internet_checksum(&data);
        // Verify: recomputing with the checksum included should yield 0.
        let mut with_checksum = data.to_vec();
        write_u16_be(&mut with_checksum, 2, checksum);
        assert_eq!(internet_checksum(&with_checksum), 0);
    }

    #[test]
    fn test_internet_checksum_odd_length() {
        // Odd-length data: last byte is zero-padded.
        let data = [0x01, 0x02, 0x03];
        let checksum = internet_checksum(&data);
        // Should not panic.
        assert!(checksum != 0 || data.iter().all(|&b| b == 0));
    }

    #[test]
    fn test_parse_ipv4_valid() {
        let mut data = vec![0u8; IP_HEADER_MIN_SIZE];
        data[0] = (IP_VERSION_4 << 4) | IP_IHL_NO_OPTIONS;
        write_u16_be(&mut data, 2, 28); // total length
        data[9] = IP_PROTO_ICMP;
        write_u32_be(&mut data, 12, 0x0100A8C0); // src
        write_u32_be(&mut data, 16, 0x0200A8C0); // dst

        let ipv4 = parse_ipv4(&data).unwrap();
        assert_eq!(ipv4.protocol, IP_PROTO_ICMP);
        assert_eq!(ipv4.src_ip, 0x0100A8C0);
        assert_eq!(ipv4.dst_ip, 0x0200A8C0);
        assert_eq!(ipv4.total_len, 28);
        assert_eq!(ipv4.header_len, 20);
    }

    #[test]
    fn test_parse_ipv4_too_short() {
        assert!(parse_ipv4(&[0u8; 10]).is_none());
    }

    #[test]
    fn test_parse_ipv4_wrong_version() {
        let mut data = vec![0u8; IP_HEADER_MIN_SIZE];
        // IPv6 version (6) in high nibble — should be rejected.
        data[0] = 0x65;
        assert!(parse_ipv4(&data).is_none());
    }

    #[test]
    fn test_format_ip() {
        // 10.0.2.15 in big-endian byte order: 0x0A00020F.
        let ip = 0x0A_00_02_0F;
        let s = alloc::format!("{:?}", format_ip(ip));
        assert_eq!(s, "10.0.2.15");
    }

    #[test]
    fn test_build_arp_reply() {
        let reply = build_arp_reply([0xAA; 6], 0x0100A8C0, [0xBB; 6], 0x0200A8C0);
        assert_eq!(reply.len(), ARP_HEADER_SIZE);
        assert_eq!(read_u16_be(&reply, 6), ARP_OP_REPLY);
        assert_eq!(&reply[8..14], &[0xAA; 6]);
        assert_eq!(&reply[18..24], &[0xBB; 6]);
    }

    #[test]
    fn test_build_arp_request() {
        let request = build_arp_request([0xAA; 6], 0x0100A8C0, 0x0200A8C0);
        assert_eq!(request.len(), ARP_HEADER_SIZE);
        assert_eq!(read_u16_be(&request, 6), ARP_OP_REQUEST);
        assert_eq!(&request[8..14], &[0xAA; 6]);
        assert_eq!(&request[18..24], &ZERO_MAC);
    }

    #[test]
    fn test_build_arp_request_target_ip() {
        let target = 0x0A00020F; // 10.0.2.15
        let request = build_arp_request([0xAA; 6], 0x0100A8C0, target);
        assert_eq!(read_u32_be(&request, 24), target);
    }

    #[test]
    fn test_arp_expiry_ticks_constant() {
        // ARP_EXPIRY_TICKS must be positive and reasonable.
        assert!(ARP_EXPIRY_TICKS > 0);
        assert_eq!(ARP_EXPIRY_TICKS, 6000);
    }

    #[test]
    fn test_arp_entry_struct() {
        let entry = ArpEntry {
            mac: [0xAA; 6],
            timestamp: 42,
        };
        assert_eq!(entry.mac, [0xAA; 6]);
        assert_eq!(entry.timestamp, 42);
    }

    #[test]
    fn test_arp_lookup_fresh_entry() {
        // Insert a fresh entry and verify lookup returns it.
        let ip = 0x0100A8C0; // 192.168.0.1
        {
            let mut table = ARP_TABLE.lock();
            table.insert(
                ip,
                ArpEntry {
                    mac: [0xAA; 6],
                    timestamp: 0,
                },
            );
        }
        // Set TICKS to a value within ARP_EXPIRY_TICKS of the entry's timestamp.
        crate::arch::x86_64::interrupts::TICKS.store(100, core::sync::atomic::Ordering::Relaxed);
        assert_eq!(arp_lookup(ip), Some([0xAA; 6]));

        // Clean up.
        ARP_TABLE.lock().remove(&ip);
    }

    #[test]
    fn test_arp_lookup_expired_entry() {
        // Insert an entry and then advance TICKS past expiry.
        let ip = 0x0200A8C0; // 192.168.0.2
        {
            let mut table = ARP_TABLE.lock();
            table.insert(
                ip,
                ArpEntry {
                    mac: [0xBB; 6],
                    timestamp: 0,
                },
            );
        }
        // Set TICKS beyond ARP_EXPIRY_TICKS.
        crate::arch::x86_64::interrupts::TICKS
            .store(ARP_EXPIRY_TICKS + 1, core::sync::atomic::Ordering::Relaxed);
        assert_eq!(arp_lookup(ip), None);

        // Entry should have been removed.
        assert!(ARP_TABLE.lock().get(&ip).is_none());
    }

    #[test]
    fn test_arp_lookup_missing_entry() {
        let ip = 0x0300A8C0; // 192.168.0.3
        assert!(arp_lookup(ip).is_none());
    }

    #[test]
    fn test_expire_arp_entries_removes_stale() {
        // Insert two entries: one stale, one fresh.
        let stale_ip = 0x0A00A8C0;
        let fresh_ip = 0x0B00A8C0;
        {
            let mut table = ARP_TABLE.lock();
            table.insert(
                stale_ip,
                ArpEntry {
                    mac: [0x11; 6],
                    timestamp: 0,
                },
            );
            table.insert(
                fresh_ip,
                ArpEntry {
                    mac: [0x22; 6],
                    timestamp: ARP_EXPIRY_TICKS - 1,
                },
            );
        }
        // Set current tick to ARP_EXPIRY_TICKS + 1.
        crate::arch::x86_64::interrupts::TICKS
            .store(ARP_EXPIRY_TICKS + 1, core::sync::atomic::Ordering::Relaxed);

        expire_arp_entries();

        let table = ARP_TABLE.lock();
        assert!(
            table.get(&stale_ip).is_none(),
            "stale entry should be removed"
        );
        assert!(
            table.get(&fresh_ip).is_some(),
            "fresh entry should be retained"
        );

        // Clean up.
        drop(table);
        ARP_TABLE.lock().remove(&fresh_ip);
    }

    #[test]
    fn test_expire_arp_entries_empty_table() {
        // Expiring on an empty table should not panic.
        ARP_TABLE.lock().clear();
        expire_arp_entries();
    }

    #[test]
    fn test_arp_expire_check_interval_constant() {
        assert!(ARP_EXPIRE_CHECK_INTERVAL > 0);
        assert_eq!(ARP_EXPIRE_CHECK_INTERVAL, 1000);
    }
}
