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
pub mod fragment;
pub mod ipv6;
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

/// `EtherType` for IPv6 (0x86DD).
const ETHERTYPE_IPV6: u16 = 0x86DD;

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

/// Get a snapshot of the ARP table for `/proc/net/arp`.
///
/// Returns a vector of (`ip_addr`, `mac_addr`) pairs.
pub fn get_arp_table() -> alloc::vec::Vec<(u32, [u8; 6])> {
    let table = ARP_TABLE.lock();
    table.iter().map(|(ip, entry)| (*ip, entry.mac)).collect()
}

// ─────────────────── Routing table ───────────────────

/// A single entry in the IP routing table.
///
/// Each entry maps a destination network (`dest & mask`) to a next-hop
/// gateway and outgoing interface. A gateway of `0` means the destination
/// is directly connected (no next hop).
///
/// The `metric` field provides an additional priority signal when two
/// routes have the same prefix length: lower metric = higher priority.
/// A metric of `0` indicates the highest possible priority.
#[derive(Debug, Clone, Copy)]
pub struct RouteEntry {
    /// Destination network address (network byte order).
    pub dest: u32,
    /// Subnet mask (network byte order, contiguous ones from MSB).
    pub mask: u32,
    /// Next-hop gateway address (network byte order); 0 = directly connected.
    pub gateway: u32,
    /// Outgoing interface index.
    pub interface: u8,
    /// Route metric (lower = higher priority). Default `0` for directly
    /// connected, `1` for static, `2` for DHCP-learned, etc.
    pub metric: u8,
}

/// Global routing table.
///
/// Protected by a spinlock. Entries are stored in no particular order;
/// `route_lookup` iterates all entries and selects the longest prefix match.
static ROUTING_TABLE: Mutex<Vec<RouteEntry>> = Mutex::new(Vec::new());

/// Add a route to the global routing table.
///
/// If a route with the same `(dest, mask)` already exists, it is replaced.
///
/// # Arguments
/// * `dest`      - Destination network (network byte order, e.g. `0x00000000` for default).
/// * `mask`      - Subnet mask (network byte order, e.g. `0x00000000` for /0).
/// * `gateway`   - Next-hop gateway (network byte order); `0` for directly connected.
/// * `interface` - Outgoing interface index.
/// * `metric`    - Route priority (lower = higher). Use `0` for directly connected.
pub fn route_add(dest: u32, mask: u32, gateway: u32, interface: u8, metric: u8) {
    let mut table = ROUTING_TABLE.lock();

    // Replace an existing entry with the same dest/mask.
    if let Some(entry) = table.iter_mut().find(|e| e.dest == dest && e.mask == mask) {
        entry.gateway = gateway;
        entry.interface = interface;
        entry.metric = metric;
        return;
    }

    table.push(RouteEntry {
        dest,
        mask,
        gateway,
        interface,
        metric,
    });
}

/// Remove a route from the routing table by destination network and mask.
///
/// Returns `true` if an entry was found and removed, `false` otherwise.
#[allow(dead_code)]
pub fn route_remove(dest: u32, mask: u32) -> bool {
    let mut table = ROUTING_TABLE.lock();
    let before = table.len();
    table.retain(|e| !(e.dest == dest && e.mask == mask));
    table.len() < before
}

/// Alias for `route_remove`. Provided for syscall API consistency.
///
/// Returns `true` if an entry was found and removed, `false` otherwise.
#[must_use]
pub fn route_delete(dest: u32, mask: u32) -> bool {
    route_remove(dest, mask)
}

/// Return a snapshot of the current routing table.
///
/// Each entry is serialized as a `RouteEntry` struct. The caller receives
/// a standalone copy so the lock is not held across allocation boundaries.
pub fn get_routing_table() -> alloc::vec::Vec<RouteEntry> {
    let table = ROUTING_TABLE.lock();
    table.clone()
}

/// Look up the best route for `dest_ip` using longest prefix match.
///
/// Iterates all routing table entries and selects the one whose
/// `(dest & mask)` matches `(dest_ip & mask)` with the highest mask
/// value (most specific match). If two entries have the same prefix
/// length, the one with the lower metric wins. If both prefix length
/// and metric are equal, the first entry (insertion order) wins.
///
/// # Returns
/// `Some((gateway, interface))` if a matching route was found,
/// `None` if no route matches (caller should drop the packet).
pub fn route_lookup(dest_ip: u32) -> Option<(u32, u8)> {
    let table = ROUTING_TABLE.lock();
    // (gateway, mask, interface, metric)
    let mut best: Option<(u32, u32, u8, u8)> = None;

    for entry in table.iter() {
        if (dest_ip & entry.mask) == (entry.dest & entry.mask) {
            match best {
                None => {
                    best = Some((entry.gateway, entry.mask, entry.interface, entry.metric));
                }
                Some((_gw, best_mask, _if, _metric)) if entry.mask > best_mask => {
                    // More specific prefix wins.
                    best = Some((entry.gateway, entry.mask, entry.interface, entry.metric));
                }
                Some((_gw, best_mask, _if, best_metric))
                    if entry.mask == best_mask && entry.metric < best_metric =>
                {
                    // Same prefix length, lower metric wins.
                    best = Some((entry.gateway, entry.mask, entry.interface, entry.metric));
                }
                _ => {}
            }
        }
    }

    best.map(|(gw, _mask, iface, _metric)| (gw, iface))
}

/// Initialize the routing table from the current DHCP network state.
///
/// Adds three routes:
/// 1. Loopback route (127.0.0.0/8) on a virtual loopback interface (interface 127).
/// 2. A directly-connected network route for the local subnet.
/// 3. A default route (0.0.0.0/0) via the DHCP gateway.
///
/// If DHCP has not completed, only the loopback and a default route to
/// `0.0.0.0` are added (which effectively means no routing). The `interface`
/// argument is the index of the physical interface that DHCP used.
///
/// Metric assignments follow the convention: directly connected = 0,
/// static routes = 1, DHCP-learned = 2, loopback = 0.
pub fn init_routing_table(interface: u8) {
    // Loopback route: 127.0.0.0/8, directly connected on virtual interface 127.
    // This ensures packets to 127.x.x.x are treated as local and not forwarded
    // to the physical network.
    const LOOPBACK_NETWORK: u32 = 0x7F00_0000; // 127.0.0.0
    const LOOPBACK_MASK: u32 = 0xFF00_0000; // 255.0.0.0  (/8)
    const LOOPBACK_INTERFACE: u8 = 127;
    route_add(LOOPBACK_NETWORK, LOOPBACK_MASK, 0, LOOPBACK_INTERFACE, 0);

    let state = dhcp::get_network_state();

    let local_ip = u32::from_be_bytes(state.ip);
    let mask = u32::from_be_bytes(state.subnet_mask);
    let gateway = u32::from_be_bytes(state.gateway);

    // Directly-connected network: local_ip & mask with gateway = 0.
    let network = local_ip & mask;
    route_add(network, mask, 0, interface, 0);

    // Default route: 0.0.0.0/0 via gateway (only if gateway is non-zero).
    if gateway != 0 {
        route_add(0, 0, gateway, interface, 2);
    }

    serial_println!(
        "[NET] Routing table initialized: loopback=127.0.0.0/8 network={:?}/{:?} gw={:?} if={}",
        format_ip(network),
        format_ip(mask),
        format_ip(gateway),
        interface
    );
}

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
pub(crate) fn internet_checksum(data: &[u8]) -> u16 {
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
fn handle_icmp(eth_src: [u8; 6], src_ip: u32, icmp_payload: &[u8]) {
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
        format_ip(src_ip)
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
    // Build header for reply: dst_ip (original dest) was our local address,
    // src_ip (original source) is the requester — we swap them.
    // We don't have dst_ip here, but we use the local IP as source.
    // The src_ip is the requester's address.
    write_u32_be(&mut ip_header, 12, local_ip()); // src = our IP
    write_u32_be(&mut ip_header, 16, src_ip); // dst = requester

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
                format_ip(src_ip),
                sent
            );
        }
        Err(e) => {
            serial_println!("[NET] ICMP reply send failed: {:?}", e);
        }
    }
}

/// Dispatch an IPv4 payload to the appropriate protocol handler.
///
/// Extracted from `handle_frame` to be callable for both unfragmented packets
/// and reassembled datagrams.
fn dispatch_ipv4_payload(eth_src: [u8; 6], protocol: u8, src_ip: u32, dst_ip: u32, data: &[u8]) {
    if protocol == IP_PROTO_ICMP {
        handle_icmp(eth_src, src_ip, data);
    } else if protocol == IP_PROTO_TCP {
        tcp::handle_tcp_packet(src_ip, dst_ip, data);
    } else if protocol == IP_PROTO_UDP {
        udp::handle_incoming_udp(src_ip, data);
        serial_println!(
            "[NET] UDP from {:?} ({} bytes)",
            format_ip(src_ip),
            data.len()
        );
    } else {
        serial_println!(
            "[NET] IPv4: protocol {} from {:?}, dropping",
            protocol,
            format_ip(src_ip)
        );
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

            // Try fragment reassembly first.
            let now =
                crate::arch::x86_64::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);
            match fragment::try_reassemble(payload, now) {
                Err(e) => {
                    serial_println!("[NET] Fragment reassembly error: {e}");
                    return;
                }
                Ok(Some(reassembled_payload)) => {
                    // A complete datagram was reassembled from fragments.
                    // Use the original IP header fields but with the reassembled payload.
                    dispatch_ipv4_payload(
                        eth.src_mac,
                        ipv4.protocol,
                        ipv4.src_ip,
                        ipv4.dst_ip,
                        &reassembled_payload,
                    );
                    return;
                }
                Ok(None) => {
                    // Not a fragment, or fragments are still buffered — handle normally.
                }
            }

            dispatch_ipv4_payload(
                eth.src_mac,
                ipv4.protocol,
                ipv4.src_ip,
                ipv4.dst_ip,
                &payload[ipv4.header_len..],
            );
        }
        ETHERTYPE_IPV6 => {
            ipv6::handle_ipv6_frame(eth.src_mac, payload);
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
    let mut last_frag_expire_tick: u64 = 0;

    loop {
        // Non-blocking poll for received frames.
        if let Some(frame) = net::receive_frame() {
            handle_frame(&frame);
        }

        // Check if the DHCP lease needs renewal (T1 = 50% of lease time).
        let mac = net::mac_address();
        dhcp::check_lease_renewal(mac, net::send_frame, net::receive_frame);

        // Periodically expire stale ARP entries and incomplete fragment sets.
        let now =
            crate::arch::x86_64::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);
        if now.saturating_sub(last_arp_expire_tick) >= ARP_EXPIRE_CHECK_INTERVAL {
            expire_arp_entries();
            last_arp_expire_tick = now;
        }
        if now.saturating_sub(last_frag_expire_tick) >= ARP_EXPIRE_CHECK_INTERVAL {
            fragment::expire_fragments(now);
            last_frag_expire_tick = now;
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
pub struct FormatIp(pub u32);

impl core::fmt::Debug for FormatIp {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let b = self.0.to_be_bytes();
        write!(f, "{}.{}.{}.{}", b[0], b[1], b[2], b[3])
    }
}

impl core::fmt::Display for FormatIp {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, f)
    }
}

// ─────────────────── Tests ───────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_write_u16_be() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        let mut buf = [0u8; 2];
        write_u16_be(&mut buf, 0, 0x1234);
        assert_eq!(buf, [0x12, 0x34]);
        assert_eq!(read_u16_be(&buf, 0), 0x1234);
    }

    #[test]
    fn test_read_write_u32_be() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        let mut buf = [0u8; 4];
        write_u32_be(&mut buf, 0, 0x01020304);
        assert_eq!(buf, [0x01, 0x02, 0x03, 0x04]);
        assert_eq!(read_u32_be(&buf, 0), 0x01020304);
    }

    #[test]
    fn test_parse_ethernet_valid() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
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
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        assert!(parse_ethernet(&[0u8; 13]).is_none());
    }

    #[test]
    fn test_build_ethernet() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
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
        let _guard = crate::TEST_SERIAL_LOCK.lock();
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
        let _guard = crate::TEST_SERIAL_LOCK.lock();
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
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        assert!(parse_arp(&[0u8; 20]).is_none());
    }

    #[test]
    fn test_parse_icmp_valid() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
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
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        assert!(parse_icmp(&[0u8; 7]).is_none());
    }

    #[test]
    fn test_internet_checksum_empty() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        // Checksum of empty data is 0xFFFF (all ones).
        assert_eq!(internet_checksum(&[]), 0xFFFF);
    }

    #[test]
    fn test_internet_checksum_known() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
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
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        // Odd-length data: last byte is zero-padded.
        let data = [0x01, 0x02, 0x03];
        let checksum = internet_checksum(&data);
        // Should not panic.
        assert!(checksum != 0 || data.iter().all(|&b| b == 0));
    }

    #[test]
    fn test_parse_ipv4_valid() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
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
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        assert!(parse_ipv4(&[0u8; 10]).is_none());
    }

    #[test]
    fn test_parse_ipv4_wrong_version() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        let mut data = vec![0u8; IP_HEADER_MIN_SIZE];
        // IPv6 version (6) in high nibble — should be rejected.
        data[0] = 0x65;
        assert!(parse_ipv4(&data).is_none());
    }

    #[test]
    fn test_format_ip() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        // 10.0.2.15 in big-endian byte order: 0x0A00020F.
        let ip = 0x0A_00_02_0F;
        let s = alloc::format!("{:?}", format_ip(ip));
        assert_eq!(s, "10.0.2.15");
    }

    #[test]
    fn test_build_arp_reply() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        let reply = build_arp_reply([0xAA; 6], 0x0100A8C0, [0xBB; 6], 0x0200A8C0);
        assert_eq!(reply.len(), ARP_HEADER_SIZE);
        assert_eq!(read_u16_be(&reply, 6), ARP_OP_REPLY);
        assert_eq!(&reply[8..14], &[0xAA; 6]);
        assert_eq!(&reply[18..24], &[0xBB; 6]);
    }

    #[test]
    fn test_build_arp_request() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        let request = build_arp_request([0xAA; 6], 0x0100A8C0, 0x0200A8C0);
        assert_eq!(request.len(), ARP_HEADER_SIZE);
        assert_eq!(read_u16_be(&request, 6), ARP_OP_REQUEST);
        assert_eq!(&request[8..14], &[0xAA; 6]);
        assert_eq!(&request[18..24], &ZERO_MAC);
    }

    #[test]
    fn test_build_arp_request_target_ip() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        let target = 0x0A00020F; // 10.0.2.15
        let request = build_arp_request([0xAA; 6], 0x0100A8C0, target);
        assert_eq!(read_u32_be(&request, 24), target);
    }

    #[test]
    fn test_arp_expiry_ticks_constant() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        // ARP_EXPIRY_TICKS must be positive and reasonable.
        assert!(ARP_EXPIRY_TICKS > 0);
        assert_eq!(ARP_EXPIRY_TICKS, 6000);
    }

    #[test]
    fn test_arp_entry_struct() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        let entry = ArpEntry {
            mac: [0xAA; 6],
            timestamp: 42,
        };
        assert_eq!(entry.mac, [0xAA; 6]);
        assert_eq!(entry.timestamp, 42);
    }

    #[test]
    fn test_arp_lookup_fresh_entry() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
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
        let _guard = crate::TEST_SERIAL_LOCK.lock();
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
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        let ip = 0x0300A8C0; // 192.168.0.3
        assert!(arp_lookup(ip).is_none());
    }

    #[test]
    fn test_expire_arp_entries_removes_stale() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
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
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        // Expiring on an empty table should not panic.
        ARP_TABLE.lock().clear();
        expire_arp_entries();
    }

    #[test]
    fn test_arp_expire_check_interval_constant() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        assert!(ARP_EXPIRE_CHECK_INTERVAL > 0);
        assert_eq!(ARP_EXPIRE_CHECK_INTERVAL, 1000);
    }
    // ─────────────────── Routing table tests ───────────────────

    /// Helper: clear the routing table before each test to avoid cross-test
    /// pollution (the table is process-global).
    fn clear_routing_table() {
        ROUTING_TABLE.lock().clear();
    }

    #[test]
    fn test_route_add_and_lookup_direct() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        clear_routing_table();

        // Add a directly-connected route: 192.168.1.0/24, gateway=0, interface=0.
        let dest: u32 = 0xC0A8_0100; // 192.168.1.0
        let mask: u32 = 0xFFFF_FF00; // 255.255.255.0
        route_add(dest, mask, 0, 0, 0);

        // A host in that subnet should match.
        let host: u32 = 0xC0A8_010A; // 192.168.1.10
        let result = route_lookup(host);
        assert_eq!(result, Some((0, 0)), "direct route should match with gw=0");

        clear_routing_table();
    }

    #[test]
    fn test_route_lookup_default_route() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        clear_routing_table();

        // Add a default route: 0.0.0.0/0 via 10.0.2.2 on interface 0.
        let gateway: u32 = 0x0A00_0202; // 10.0.2.2
        route_add(0, 0, gateway, 0, 2);

        // Any IP should match the default route.
        let remote: u32 = 0xCBCB_0101; // 203.203.1.1
        let result = route_lookup(remote);
        assert_eq!(
            result,
            Some((gateway, 0)),
            "default route should match any destination"
        );

        clear_routing_table();
    }

    #[test]
    fn test_route_lookup_longest_prefix_match() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        clear_routing_table();

        // Add two overlapping routes:
        //   10.0.0.0/8     via 10.0.0.1 (interface 0)  -- less specific
        //   10.0.2.0/24    via 0 (directly connected, interface 1) -- more specific
        let broad_dest: u32 = 0x0A00_0000; // 10.0.0.0
        let broad_mask: u32 = 0xFF00_0000; // 255.0.0.0 (/8)
        let broad_gw: u32 = 0x0A00_0001; // 10.0.0.1

        let specific_dest: u32 = 0x0A00_0200; // 10.0.2.0
        let specific_mask: u32 = 0xFFFF_FF00; // 255.255.255.0 (/24)

        route_add(broad_dest, broad_mask, broad_gw, 0, 0);
        route_add(specific_dest, specific_mask, 0, 1, 1);

        // 10.0.2.15 matches both routes; /24 should win (longest prefix).
        let host: u32 = 0x0A00_020F; // 10.0.2.15
        let result = route_lookup(host);
        assert_eq!(
            result,
            Some((0, 1)),
            "/24 route should win over /8 for 10.0.2.15"
        );

        // 10.1.0.5 matches only the /8 route.
        let other_host: u32 = 0x0A01_0005; // 10.1.0.5
        let result2 = route_lookup(other_host);
        assert_eq!(
            result2,
            Some((broad_gw, 0)),
            "/8 route should match 10.1.0.5"
        );

        clear_routing_table();
    }

    #[test]
    fn test_route_lookup_no_match() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        clear_routing_table();

        // Only add a 192.168.1.0/24 route.
        route_add(0xC0A8_0100, 0xFFFF_FF00, 0, 0, 0);

        // A host outside that subnet should return None.
        let remote: u32 = 0x0A00_020F; // 10.0.2.15
        assert!(
            route_lookup(remote).is_none(),
            "host outside the route network should not match"
        );

        clear_routing_table();
    }

    #[test]
    fn test_route_add_replaces_duplicate() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        clear_routing_table();

        // Add a route, then add the same dest/mask with a different gateway.
        route_add(0xC0A8_0100, 0xFFFF_FF00, 0, 0, 0);
        route_add(0xC0A8_0100, 0xFFFF_FF00, 0xC0A8_0101, 1, 1);

        let table = ROUTING_TABLE.lock();
        let matching: Vec<_> = table
            .iter()
            .filter(|e| e.dest == 0xC0A8_0100 && e.mask == 0xFFFF_FF00)
            .collect();
        assert_eq!(matching.len(), 1, "duplicate dest/mask should be replaced");
        assert_eq!(matching[0].gateway, 0xC0A8_0101);
        assert_eq!(matching[0].interface, 1);
        drop(table);

        clear_routing_table();
    }

    #[test]
    fn test_route_remove() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        clear_routing_table();

        route_add(0xC0A8_0100, 0xFFFF_FF00, 0, 0, 0);
        assert!(route_remove(0xC0A8_0100, 0xFFFF_FF00));
        assert!(
            route_lookup(0xC0A8_010A).is_none(),
            "removed route should not match"
        );

        // Removing again should return false.
        assert!(!route_remove(0xC0A8_0100, 0xFFFF_FF00));

        clear_routing_table();
    }

    #[test]
    fn test_route_entry_struct() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        let entry = RouteEntry {
            dest: 0xC0A8_0100,
            mask: 0xFFFF_FF00,
            gateway: 0xC0A8_0101,
            interface: 0,
            metric: 0,
        };
        assert_eq!(entry.dest, 0xC0A8_0100);
        assert_eq!(entry.mask, 0xFFFF_FF00);
        assert_eq!(entry.gateway, 0xC0A8_0101);
        assert_eq!(entry.interface, 0);
        assert_eq!(entry.metric, 0);
    }

    #[test]
    fn test_route_lookup_empty_table() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        clear_routing_table();
        assert!(
            route_lookup(0x0A00_020F).is_none(),
            "empty table should return None"
        );
    }
    #[test]
    fn test_route_lookup_metric_preference() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        clear_routing_table();

        // Two routes with the same prefix (/24). The one with lower metric wins.
        route_add(0xC0A8_0100, 0xFFFF_FF00, 0x0A00_0001, 0, 10);
        route_add(0xC0A8_0100, 0xFFFF_FF00, 0x0A00_0002, 1, 1);

        // Both match but metric=1 should win over metric=10.
        let host: u32 = 0xC0A8_010A; // 192.168.1.10
        let result = route_lookup(host);
        assert_eq!(
            result,
            Some((0x0A00_0002, 1)),
            "lower metric route (1) should win over higher metric (10)"
        );

        clear_routing_table();
    }

    #[test]
    fn test_route_delete_alias() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        clear_routing_table();

        route_add(0x0A00_0000, 0xFF00_0000, 0x0A00_0001, 0, 1);
        assert!(route_delete(0x0A00_0000, 0xFF00_0000));
        assert!(
            route_lookup(0x0A00_0002).is_none(),
            "deleted route should not match"
        );

        clear_routing_table();
    }

    #[test]
    fn test_get_routing_table_snapshot() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        clear_routing_table();

        route_add(0xC0A8_0100, 0xFFFF_FF00, 0, 0, 0);
        route_add(0x0A00_0000, 0xFF00_0000, 0x0A00_0001, 1, 2);

        let snapshot = get_routing_table();
        assert_eq!(snapshot.len(), 2);
        assert!(snapshot.iter().any(|e| e.dest == 0xC0A8_0100));
        assert!(snapshot.iter().any(|e| e.dest == 0x0A00_0000));

        clear_routing_table();
    }
}
