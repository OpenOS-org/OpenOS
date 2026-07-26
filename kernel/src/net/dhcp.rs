//! DHCP client for automatic IP configuration (RFC 2131, RFC 2132).
//!
//! Implements the DHCP DISCOVER / OFFER / REQUEST / ACK four-way handshake
//! to obtain an IPv4 address from QEMU's built-in DHCP server.
//!
//! ## Protocol Flow
//!
//! ```text
//!   Client                          Server (QEMU)
//!     │                                │
//!     │  DHCP DISCOVER (broadcast)     │
//!     │  ───────────────────────────→  │
//!     │                                │
//!     │  DHCP OFFER                    │
//!     │  ←───────────────────────────  │
//!     │                                │
//!     │  DHCP REQUEST (broadcast)      │
//!     │  ───────────────────────────→  │
//!     │                                │
//!     │  DHCP ACK                      │
//!     │  ←───────────────────────────  │
//!     │                                │
//!     │  (IP address configured)       │
//! ```
//!
//! ## DHCP Message Format (RFC 2131)
//!
//! ```text
//!  0                   1                   2                   3
//!  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |     op (1)    |   htype (1)   |    hlen (1)   |   hops (1)    |
//! +---------------+---------------+---------------+---------------+
//! |                            xid (4)                            |
//! +-------------------------------+-------------------------------+
//! |           secs (2)            |           flags (2)           |
//! +-------------------------------+-------------------------------+
//! |                          ciaddr (4)                           |
//! +---------------------------------------------------------------+
//! |                          yiaddr (4)                           |
//! +---------------------------------------------------------------+
//! |                          siaddr (4)                           |
//! +---------------------------------------------------------------+
//! |                          giaddr (4)                           |
//! +---------------------------------------------------------------+
//! |                                                               |
//! |                          chaddr (16)                          |
//! |                                                               |
//! +---------------------------------------------------------------+
//! |                                                               |
//! |                            sname (64)                         |
//! +---------------------------------------------------------------+
//! |                                                               |
//! |                            file (128)                         |
//! +---------------------------------------------------------------+
//! |                          magic cookie (4)                     |
//! +---------------------------------------------------------------+
//! |                          options (variable)                   |
//! +---------------------------------------------------------------+
//! ```
//!
//! ## Network State
//!
//! After a successful DHCP exchange, the global `NETWORK_STATE` holds the
//! assigned IP address, subnet mask, gateway, and DNS server. Other kernel
//! subsystems can query this state for address resolution.

use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use spin::Mutex;

use super::udp;

// ---------------------------------------------------------------------------
// DHCP Constants (RFC 2131, RFC 2132)
// ---------------------------------------------------------------------------

/// DHCP message op: boot request (client → server).
const OP_BOOT_REQUEST: u8 = 1;

/// DHCP message op: boot reply (server → client).
#[allow(dead_code)]
const OP_BOOT_REPLY: u8 = 2;

/// Hardware type: Ethernet (10Mb).
const HTYPE_ETHERNET: u8 = 1;

/// Hardware address length for Ethernet (6 bytes MAC).
const HLEN_ETHERNET: u8 = 6;

/// DHCP magic cookie bytes (RFC 2131 §3).
const MAGIC_COOKIE: [u8; 4] = [99, 130, 83, 99];

/// DHCP option: pad (0).
const OPT_PAD: u8 = 0;

/// DHCP option: end (255).
const OPT_END: u8 = 255;

/// DHCP option: message type (53).
const OPT_MESSAGE_TYPE: u8 = 53;

/// DHCP option: requested IP address (50).
const OPT_REQUESTED_IP: u8 = 50;

/// DHCP option: server identifier (54).
const OPT_SERVER_ID: u8 = 54;

/// DHCP option: parameter request list (55).
const OPT_PARAM_REQUEST_LIST: u8 = 55;

/// DHCP option: subnet mask (1).
const OPT_SUBNET_MASK: u8 = 1;

/// DHCP option: router (3).
const OPT_ROUTER: u8 = 3;

/// DHCP option: DNS server (6).
const OPT_DNS_SERVER: u8 = 6;

/// DHCP option: IP address lease time (51).
#[allow(dead_code)]
const OPT_LEASE_TIME: u8 = 51;

/// DHCP DISCOVER message type.
const MSG_DISCOVER: u8 = 1;

/// DHCP OFFER message type.
const MSG_OFFER: u8 = 2;

/// DHCP REQUEST message type.
const MSG_REQUEST: u8 = 3;

/// DHCP ACK message type.
const MSG_ACK: u8 = 5;

/// DHCP fixed header size (without options).
const DHCP_HEADER_SIZE: usize = 236;

/// DHCP minimum packet size (header + magic cookie + end option).
/// DHCP packets are padded to at least this size (RFC 2131 §4.1).
const DHCP_MIN_PACKET_SIZE: usize = 300;

/// Broadcast IPv4 address (255.255.255.255).
const IP_BROADCAST: [u8; 4] = [255, 255, 255, 255];

/// All-zeros IPv4 address (0.0.0.0).
const IP_ZERO: [u8; 4] = [0, 0, 0, 0];

/// DHCP server UDP port.
const DHCP_SERVER_PORT: u16 = 67;

/// DHCP client UDP port.
const DHCP_CLIENT_PORT: u16 = 68;

/// Transaction ID for the current DHCP exchange.
/// We use a fixed value for simplicity; a real implementation would randomize.
const DHCP_XID: u32 = 0x39_A3_00_5A;

// ---------------------------------------------------------------------------
// DHCP Message
// ---------------------------------------------------------------------------

/// A parsed DHCP message.
///
/// Represents both requests (client → server) and replies (server → client).
/// The fixed header fields are stored directly; options are parsed on demand.
struct DhcpMessage {
    /// Operation: 1 = request, 2 = reply.
    op: u8,
    /// Hardware address type (1 = Ethernet).
    htype: u8,
    /// Hardware address length (6 for Ethernet).
    hlen: u8,
    /// Gateway hops.
    hops: u8,
    /// Transaction ID.
    xid: u32,
    /// Seconds elapsed since client began acquisition.
    secs: u16,
    /// Flags (bit 15 = broadcast).
    flags: u16,
    /// Client IP address (filled if client is in BOUND state).
    ciaddr: [u8; 4],
    /// Your (client) IP address (filled by server in OFFER/ACK).
    yiaddr: [u8; 4],
    /// Next server IP address.
    siaddr: [u8; 4],
    /// Relay agent IP address.
    giaddr: [u8; 4],
    /// Client hardware address (16 bytes, padded for Ethernet).
    chaddr: [u8; 16],
    /// Raw options bytes (after magic cookie).
    options: Vec<u8>,
}

// ---------------------------------------------------------------------------
// DHCP Option Parser
// ---------------------------------------------------------------------------

/// An iterator over DHCP options in a message.
///
/// Walks the options buffer, yielding (`option_code`, `option_data`) pairs.
/// The PAD option is skipped; the END option terminates iteration.
struct DhcpOptionIter<'a> {
    data: &'a [u8],
    pos: usize,
}

/// A single DHCP option.
struct DhcpOption<'a> {
    code: u8,
    data: &'a [u8],
}

impl<'a> DhcpOptionIter<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
}

impl<'a> Iterator for DhcpOptionIter<'a> {
    type Item = DhcpOption<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.pos < self.data.len() {
            let code = self.data[self.pos];

            // PAD option: skip.
            if code == OPT_PAD {
                self.pos += 1;
                continue;
            }

            // END option: stop.
            if code == OPT_END {
                return None;
            }

            // Option must have at least a length byte.
            if self.pos + 1 >= self.data.len() {
                return None;
            }

            let len = self.data[self.pos + 1] as usize;
            let data_start = self.pos + 2;

            // Bounds check: option data must fit within the buffer.
            if data_start + len > self.data.len() {
                return None;
            }

            let data = &self.data[data_start..data_start + len];
            self.pos = data_start + len;

            return Some(DhcpOption { code, data });
        }
        None
    }
}

// ---------------------------------------------------------------------------
// DHCP Message Construction
// ---------------------------------------------------------------------------

impl DhcpMessage {
    /// Create a new DHCP DISCOVER or REQUEST message from the client.
    ///
    /// Populates the fixed header with the client's MAC address and a
    /// transaction ID. Options are added separately via `add_option`.
    fn new_request(mac: [u8; 6], xid: u32) -> Self {
        let mut chaddr = [0u8; 16];
        chaddr[..6].copy_from_slice(&mac);

        Self {
            op: OP_BOOT_REQUEST,
            htype: HTYPE_ETHERNET,
            hlen: HLEN_ETHERNET,
            hops: 0,
            xid,
            secs: 0,
            flags: 0x8000, // Broadcast flag (RFC 2131 §4.4.1).
            ciaddr: IP_ZERO,
            yiaddr: IP_ZERO,
            siaddr: IP_ZERO,
            giaddr: IP_ZERO,
            chaddr,
            options: Vec::new(),
        }
    }

    /// Serialize the DHCP message to bytes.
    ///
    /// Produces a complete DHCP packet: 236-byte fixed header + magic cookie +
    /// options, padded to at least `DHCP_MIN_PACKET_SIZE`.
    fn to_bytes(&self) -> Vec<u8> {
        let total = DHCP_HEADER_SIZE + 4 + self.options.len();
        let padded = total.max(DHCP_MIN_PACKET_SIZE);
        let mut buf = vec![0u8; padded];

        buf[0] = self.op;
        buf[1] = self.htype;
        buf[2] = self.hlen;
        buf[3] = self.hops;
        buf[4..8].copy_from_slice(&self.xid.to_be_bytes());
        buf[8..10].copy_from_slice(&self.secs.to_be_bytes());
        buf[10..12].copy_from_slice(&self.flags.to_be_bytes());
        buf[12..16].copy_from_slice(&self.ciaddr);
        buf[16..20].copy_from_slice(&self.yiaddr);
        buf[20..24].copy_from_slice(&self.siaddr);
        buf[24..28].copy_from_slice(&self.giaddr);
        buf[28..44].copy_from_slice(&self.chaddr);
        // sname (64 bytes) and file (128 bytes) are zeroed.

        // Magic cookie at offset 236.
        let cookie_offset = DHCP_HEADER_SIZE;
        buf[cookie_offset..cookie_offset + 4].copy_from_slice(&MAGIC_COOKIE);

        // Options after magic cookie.
        let opt_offset = cookie_offset + 4;
        let opt_end = opt_offset + self.options.len();
        buf[opt_offset..opt_end].copy_from_slice(&self.options);

        buf
    }
}

// ---------------------------------------------------------------------------
// DHCP Client
// ---------------------------------------------------------------------------

/// Global network configuration state.
///
/// After a successful DHCP handshake, this holds the assigned addresses.
/// Other subsystems (ARP, TCP) read from here for address resolution.
pub struct NetworkState {
    /// Assigned IPv4 address.
    pub ip: [u8; 4],
    /// Subnet mask.
    pub subnet_mask: [u8; 4],
    /// Default gateway address.
    pub gateway: [u8; 4],
    /// DNS server address.
    pub dns: [u8; 4],
    /// Whether the network has been configured via DHCP.
    pub configured: bool,
}

/// Global network state (protected by a spinlock).
///
/// Initialized with zeros; populated by `dhcp_negotiate` after a
/// successful DISCOVER / OFFER / REQUEST / ACK exchange.
static NETWORK_STATE: Mutex<NetworkState> = Mutex::new(NetworkState {
    ip: [0, 0, 0, 0],
    subnet_mask: [255, 255, 255, 0],
    gateway: [0, 0, 0, 0],
    dns: [0, 0, 0, 0],
    configured: false,
});

/// Whether the DHCP client is currently negotiating.
///
/// Prevents concurrent negotiations (only one DHCP exchange at a time).
static NEGOTIATING: AtomicBool = AtomicBool::new(false);

/// Get the current network configuration.
///
/// Returns a copy of the global `NetworkState`. If DHCP has not completed,
/// all fields are zero except `subnet_mask` (defaults to 255.255.255.0).
pub fn get_network_state() -> NetworkState {
    let state = NETWORK_STATE.lock();
    NetworkState {
        ip: state.ip,
        subnet_mask: state.subnet_mask,
        gateway: state.gateway,
        dns: state.dns,
        configured: state.configured,
    }
}

// ---------------------------------------------------------------------------
// DHCP Message Sending
// ---------------------------------------------------------------------------

/// Build a DHCP DISCOVER message.
///
/// Creates a DHCP DISCOVER packet wrapped in UDP/IP, ready for Ethernet
/// framing and transmission. The DISCOVER message requests any available
/// IP address from the server.
///
/// # Arguments
///
/// * `mac` - Client's MAC address (6 bytes).
///
/// # Returns
///
/// A `Vec<u8>` containing the IPv4 + UDP + DHCP packet bytes.
fn build_discover(mac: [u8; 6]) -> Vec<u8> {
    let mut msg = DhcpMessage::new_request(mac, DHCP_XID);

    // Message type option: DISCOVER.
    msg.options
        .extend_from_slice(&[OPT_MESSAGE_TYPE, 1, MSG_DISCOVER]);

    // Parameter request list: subnet mask, router, DNS.
    msg.options.extend_from_slice(&[
        OPT_PARAM_REQUEST_LIST,
        3,
        OPT_SUBNET_MASK,
        OPT_ROUTER,
        OPT_DNS_SERVER,
    ]);

    // End option.
    msg.options.push(OPT_END);

    let dhcp_payload = msg.to_bytes();

    // Wrap in UDP from 0.0.0.0:68 → 255.255.255.255:67.
    udp::build_udp(
        IP_ZERO,
        IP_BROADCAST,
        DHCP_CLIENT_PORT,
        DHCP_SERVER_PORT,
        &dhcp_payload,
    )
}

/// Build a DHCP REQUEST message.
///
/// Creates a DHCP REQUEST packet that specifies the offered IP address
/// and server identifier. Sent as a broadcast to confirm the lease.
///
/// # Arguments
///
/// * `mac` - Client's MAC address (6 bytes).
/// * `offered_ip` - The IP address offered by the server in the OFFER.
/// * `server_ip` - The DHCP server's IP address (from the OFFER).
///
/// # Returns
///
/// A `Vec<u8>` containing the IPv4 + UDP + DHCP packet bytes.
fn build_request(mac: [u8; 6], offered_ip: [u8; 4], server_ip: [u8; 4]) -> Vec<u8> {
    let mut msg = DhcpMessage::new_request(mac, DHCP_XID);

    // Message type option: REQUEST.
    msg.options
        .extend_from_slice(&[OPT_MESSAGE_TYPE, 1, MSG_REQUEST]);

    // Requested IP address option.
    msg.options.extend_from_slice(&[OPT_REQUESTED_IP, 4]);
    msg.options.extend_from_slice(&offered_ip);

    // Server identifier option.
    msg.options.extend_from_slice(&[OPT_SERVER_ID, 4]);
    msg.options.extend_from_slice(&server_ip);

    // Parameter request list: subnet mask, router, DNS.
    msg.options.extend_from_slice(&[
        OPT_PARAM_REQUEST_LIST,
        3,
        OPT_SUBNET_MASK,
        OPT_ROUTER,
        OPT_DNS_SERVER,
    ]);

    // End option.
    msg.options.push(OPT_END);

    let dhcp_payload = msg.to_bytes();

    // Wrap in UDP from 0.0.0.0:68 → 255.255.255.255:67.
    udp::build_udp(
        IP_ZERO,
        IP_BROADCAST,
        DHCP_CLIENT_PORT,
        DHCP_SERVER_PORT,
        &dhcp_payload,
    )
}

// ---------------------------------------------------------------------------
// DHCP Message Parsing
// ---------------------------------------------------------------------------

/// Parse a raw DHCP packet into a `DhcpMessage`.
///
/// Validates the minimum length, op code, hardware type, and magic cookie
/// before extracting the fixed header fields and options.
///
/// # Arguments
///
/// * `data` - Raw DHCP packet bytes (without IP/UDP headers).
///
/// # Returns
///
/// `Some(DhcpMessage)` if the packet is a valid DHCP reply, `None` otherwise.
fn parse_dhcp(data: &[u8]) -> Option<DhcpMessage> {
    // Minimum: 236 (header) + 4 (magic cookie) + 1 (end option).
    if data.len() < DHCP_HEADER_SIZE + 5 {
        return None;
    }

    // Must be a boot reply from the server.
    if data[0] != OP_BOOT_REPLY {
        return None;
    }

    // Must be Ethernet.
    if data[1] != HTYPE_ETHERNET || data[2] != HLEN_ETHERNET {
        return None;
    }

    // Verify magic cookie.
    let cookie_offset = DHCP_HEADER_SIZE;
    if data[cookie_offset..cookie_offset + 4] != MAGIC_COOKIE {
        return None;
    }

    let xid = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let secs = u16::from_be_bytes([data[8], data[9]]);
    let flags = u16::from_be_bytes([data[10], data[11]]);

    let mut ciaddr = [0u8; 4];
    ciaddr.copy_from_slice(&data[12..16]);
    let mut yiaddr = [0u8; 4];
    yiaddr.copy_from_slice(&data[16..20]);
    let mut siaddr = [0u8; 4];
    siaddr.copy_from_slice(&data[20..24]);
    let mut giaddr = [0u8; 4];
    giaddr.copy_from_slice(&data[24..28]);
    let mut chaddr = [0u8; 16];
    chaddr.copy_from_slice(&data[28..44]);

    // Options start after magic cookie.
    let opt_start = cookie_offset + 4;
    let options = if opt_start < data.len() {
        data[opt_start..].to_vec()
    } else {
        Vec::new()
    };

    Some(DhcpMessage {
        op: data[0],
        htype: data[1],
        hlen: data[2],
        hops: data[3],
        xid,
        secs,
        flags,
        ciaddr,
        yiaddr,
        siaddr,
        giaddr,
        chaddr,
        options,
    })
}

/// Extract the DHCP message type from a parsed message's options.
///
/// Searches the options buffer for option 53 (DHCP Message Type).
/// Returns `None` if the option is missing or malformed.
fn get_message_type(msg: &DhcpMessage) -> Option<u8> {
    let iter = DhcpOptionIter::new(&msg.options);
    for opt in iter {
        if opt.code == OPT_MESSAGE_TYPE && opt.data.len() == 1 {
            return Some(opt.data[0]);
        }
    }
    None
}

/// Extract a specific option's data from a parsed DHCP message.
///
/// Searches the options buffer for the given option code.
/// Returns the option data slice if found, `None` otherwise.
fn get_option(msg: &DhcpMessage, code: u8) -> Option<&[u8]> {
    let iter = DhcpOptionIter::new(&msg.options);
    for opt in iter {
        if opt.code == code {
            return Some(opt.data);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// DHCP Offer / ACK Processing
// ---------------------------------------------------------------------------

/// Process a DHCP OFFER message.
///
/// Validates that the OFFER is for our transaction (xid match) and extracts
/// the offered IP address and server identifier.
///
/// # Arguments
///
/// * `msg` - Parsed DHCP OFFER message.
///
/// # Returns
///
/// `Some((offered_ip, server_ip))` if the OFFER is valid, `None` otherwise.
fn process_offer(msg: &DhcpMessage) -> Option<([u8; 4], [u8; 4])> {
    // Verify transaction ID.
    if msg.xid != DHCP_XID {
        crate::serial_println!("[DHCP] OFFER xid mismatch: got {:#x}", msg.xid);
        return None;
    }

    // Verify message type is OFFER.
    let msg_type = get_message_type(msg)?;
    if msg_type != MSG_OFFER {
        crate::serial_println!("[DHCP] Expected OFFER, got type {}", msg_type);
        return None;
    }

    // Extract offered IP address (yiaddr field).
    let offered_ip = msg.yiaddr;
    if offered_ip == IP_ZERO {
        crate::serial_println!("[DHCP] OFFER has zero yiaddr");
        return None;
    }

    // Extract server identifier from options.
    let server_ip = match get_option(msg, OPT_SERVER_ID) {
        Some(data) if data.len() == 4 => {
            let mut ip = [0u8; 4];
            ip.copy_from_slice(data);
            ip
        }
        _ => {
            // Fall back to siaddr if server identifier option is missing.
            crate::serial_println!("[DHCP] No server identifier option, using siaddr");
            msg.siaddr
        }
    };

    crate::serial_println!(
        "[DHCP] OFFER: offered IP {}.{}.{}.{}, server {}.{}.{}.{}",
        offered_ip[0],
        offered_ip[1],
        offered_ip[2],
        offered_ip[3],
        server_ip[0],
        server_ip[1],
        server_ip[2],
        server_ip[3]
    );

    Some((offered_ip, server_ip))
}

/// Process a DHCP ACK message.
///
/// Validates the ACK and updates the global `NETWORK_STATE` with the
/// assigned IP address, subnet mask, gateway, and DNS server.
///
/// # Arguments
///
/// * `msg` - Parsed DHCP ACK message.
///
/// # Returns
///
/// `true` if the ACK is valid and the network state was updated, `false` otherwise.
fn process_ack(msg: &DhcpMessage) -> bool {
    // Verify transaction ID.
    if msg.xid != DHCP_XID {
        crate::serial_println!("[DHCP] ACK xid mismatch: got {:#x}", msg.xid);
        return false;
    }

    // Verify message type is ACK.
    let Some(msg_type) = get_message_type(msg) else {
        crate::serial_println!("[DHCP] ACK missing message type option");
        return false;
    };
    if msg_type != MSG_ACK {
        crate::serial_println!("[DHCP] Expected ACK, got type {}", msg_type);
        return false;
    }

    // The assigned IP is in yiaddr.
    let ip = msg.yiaddr;
    if ip == IP_ZERO {
        crate::serial_println!("[DHCP] ACK has zero yiaddr");
        return false;
    }

    // Extract optional parameters.
    let mut state = NETWORK_STATE.lock();
    state.ip = ip;

    // Subnet mask (option 1).
    if let Some(data) = get_option(msg, OPT_SUBNET_MASK) {
        if data.len() == 4 {
            state.subnet_mask.copy_from_slice(data);
        }
    }

    // Router / gateway (option 3, first 4 bytes).
    if let Some(data) = get_option(msg, OPT_ROUTER) {
        if data.len() >= 4 {
            state.gateway.copy_from_slice(&data[..4]);
        }
    }

    // DNS server (option 6, first 4 bytes).
    if let Some(data) = get_option(msg, OPT_DNS_SERVER) {
        if data.len() >= 4 {
            state.dns.copy_from_slice(&data[..4]);
        }
    }

    state.configured = true;

    // Copy values for logging before dropping the lock.
    let mask = state.subnet_mask;
    let gw = state.gateway;
    let dns = state.dns;
    drop(state);

    crate::serial_println!(
        "[DHCP] ACK: IP {}.{}.{}.{}, mask {}.{}.{}.{}, gw {}.{}.{}.{}, dns {}.{}.{}.{}",
        ip[0],
        ip[1],
        ip[2],
        ip[3],
        mask[0],
        mask[1],
        mask[2],
        mask[3],
        gw[0],
        gw[1],
        gw[2],
        gw[3],
        dns[0],
        dns[1],
        dns[2],
        dns[3]
    );

    true
}

// ---------------------------------------------------------------------------
// Ethernet Frame Construction
// ---------------------------------------------------------------------------

/// Ethernet frame type for IPv4.
const ETHERTYPE_IPV4: u16 = 0x0800;

/// Ethernet broadcast destination address.
const ETH_BROADCAST: [u8; 6] = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];

/// Build an Ethernet frame wrapping an IPv4 packet.
///
/// # Arguments
///
/// * `src_mac` - Source MAC address.
/// * `dst_mac` - Destination MAC address (broadcast for DHCP).
/// * `payload` - IPv4 packet bytes.
///
/// # Returns
///
/// A `Vec<u8>` containing the complete Ethernet frame (14-byte header + payload).
fn build_ethernet_frame(src_mac: [u8; 6], dst_mac: [u8; 6], payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(14 + payload.len());
    frame.extend_from_slice(&dst_mac);
    frame.extend_from_slice(&src_mac);
    frame.extend_from_slice(&ETHERTYPE_IPV4.to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

// ---------------------------------------------------------------------------
// Public API: DHCP Negotiation
// ---------------------------------------------------------------------------

/// Run the full DHCP handshake to obtain an IP address.
///
/// Sends a DHCP DISCOVER, waits for an OFFER, sends a REQUEST, and waits
/// for an ACK. On success, the global `NETWORK_STATE` is updated with the
/// assigned address.
///
/// # Arguments
///
/// * `mac` - The client's MAC address (from the NIC driver).
/// * `send_ethernet` - Function to send a raw Ethernet frame.
/// * `receive_ethernet` - Function to receive a raw Ethernet frame (non-blocking).
///
/// # Returns
///
/// `true` if the handshake completed and an IP was assigned, `false` otherwise.
///
/// # Limitations
///
/// - Uses a fixed transaction ID (not randomized).
/// - Implements only the SELECTING state (no renewal/rebinding).
/// - Does not implement DHCP INFORM or RELEASE.
/// - The receive loop has a fixed retry count, not a real timeout.
pub fn dhcp_negotiate<F, R>(mac: [u8; 6], send_ethernet: F, receive_ethernet: R) -> bool
where
    F: Fn(&[u8]) -> Result<usize, crate::drivers::net::NetError>,
    R: Fn() -> Option<Vec<u8>>,
{
    // Prevent concurrent negotiations.
    if NEGOTIATING.swap(true, Ordering::Acquire) {
        crate::serial_println!("[DHCP] Negotiation already in progress");
        return false;
    }

    crate::serial_println!("[DHCP] Starting DHCP negotiation...");

    // --- Step 1: Send DHCP DISCOVER ---
    let discover_packet = build_discover(mac);
    let discover_frame = build_ethernet_frame(mac, ETH_BROADCAST, &discover_packet);

    if let Err(e) = send_ethernet(&discover_frame) {
        crate::serial_println!("[DHCP] Failed to send DISCOVER: {:?}", e);
        NEGOTIATING.store(false, Ordering::Release);
        return false;
    }
    crate::serial_println!("[DHCP] DISCOVER sent");

    // --- Step 2: Wait for DHCP OFFER ---
    // Try receiving frames for up to a fixed number of iterations.
    // In a real kernel this would use a timer or interrupt-driven receive.
    let max_retries = 1000;
    let mut offered_ip = None;
    let mut server_ip = None;

    for i in 0..max_retries {
        if let Some(frame) = receive_ethernet() {
            if let Some(dhcp_msg) = extract_dhcp_from_frame(&frame) {
                if let Some(msg) = parse_dhcp(&dhcp_msg) {
                    if let Some((offered, server)) = process_offer(&msg) {
                        offered_ip = Some(offered);
                        server_ip = Some(server);
                        break;
                    }
                }
            }
        }

        // Brief pause between polls (prevents busy-waiting in a tight loop).
        if i % 100 == 0 {
            for _ in 0..10_000 {
                core::hint::spin_loop();
            }
        }
    }

    let (Some(offered_ip), Some(server_ip)) = (offered_ip, server_ip) else {
        crate::serial_println!("[DHCP] No OFFER received");
        NEGOTIATING.store(false, Ordering::Release);
        return false;
    };

    // --- Step 3: Send DHCP REQUEST ---
    let request_packet = build_request(mac, offered_ip, server_ip);
    let request_frame = build_ethernet_frame(mac, ETH_BROADCAST, &request_packet);

    if let Err(e) = send_ethernet(&request_frame) {
        crate::serial_println!("[DHCP] Failed to send REQUEST: {:?}", e);
        NEGOTIATING.store(false, Ordering::Release);
        return false;
    }
    crate::serial_println!("[DHCP] REQUEST sent");

    // --- Step 4: Wait for DHCP ACK ---
    let mut ack_received = false;
    for i in 0..max_retries {
        if let Some(frame) = receive_ethernet() {
            if let Some(dhcp_msg) = extract_dhcp_from_frame(&frame) {
                if let Some(msg) = parse_dhcp(&dhcp_msg) {
                    if process_ack(&msg) {
                        ack_received = true;
                        break;
                    }
                }
            }
        }

        if i % 100 == 0 {
            for _ in 0..10_000 {
                core::hint::spin_loop();
            }
        }
    }

    NEGOTIATING.store(false, Ordering::Release);

    if ack_received {
        crate::serial_println!("[DHCP] Negotiation complete");
        true
    } else {
        crate::serial_println!("[DHCP] No ACK received");
        false
    }
}

// ---------------------------------------------------------------------------
// Frame Parsing Helpers
// ---------------------------------------------------------------------------

/// Extract DHCP payload from an Ethernet frame.
///
/// Validates the Ethernet header (IPv4 type), skips the IP and UDP headers,
/// and returns the raw DHCP packet bytes.
///
/// # Arguments
///
/// * `frame` - Complete Ethernet frame (14-byte header + IP + UDP + DHCP).
///
/// # Returns
///
/// `Some(dhcp_bytes)` if the frame contains a valid UDP packet on the
/// DHCP client port (68), `None` otherwise.
fn extract_dhcp_from_frame(frame: &[u8]) -> Option<Vec<u8>> {
    // Ethernet header: 14 bytes minimum.
    if frame.len() < 14 {
        return None;
    }

    // Check EtherType (bytes 12..14).
    let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
    if ethertype != ETHERTYPE_IPV4 {
        return None;
    }

    // IPv4 header starts at offset 14.
    let ip_start = 14;
    if frame.len() < ip_start + 20 {
        return None;
    }

    // Verify IPv4 version (4) and IHL (>= 5).
    let version_ihl = frame[ip_start];
    if (version_ihl >> 4) != 4 {
        return None;
    }
    let ihl = (version_ihl & 0x0F) as usize;
    if ihl < 5 {
        return None;
    }
    let ip_header_len = ihl * 4;

    // Verify UDP protocol.
    let protocol = frame[ip_start + 9];
    if protocol != 17 {
        // 17 = UDP
        return None;
    }

    // Total IP length.
    let ip_total_len = u16::from_be_bytes([frame[ip_start + 2], frame[ip_start + 3]]) as usize;

    // UDP header starts after IP header.
    let udp_start = ip_start + ip_header_len;
    if frame.len() < udp_start + 8 {
        return None;
    }

    // Verify destination port is DHCP client port (68).
    let dst_port = u16::from_be_bytes([frame[udp_start + 2], frame[udp_start + 3]]);
    if dst_port != DHCP_CLIENT_PORT {
        return None;
    }

    // UDP length.
    let udp_length = u16::from_be_bytes([frame[udp_start + 4], frame[udp_start + 5]]) as usize;
    if udp_length < 8 {
        return None;
    }

    // DHCP payload starts after UDP header.
    let dhcp_start = udp_start + 8;
    let dhcp_len = udp_length - 8;

    if frame.len() < dhcp_start + dhcp_len {
        return None;
    }

    Some(frame[dhcp_start..dhcp_start + dhcp_len].to_vec())
}
