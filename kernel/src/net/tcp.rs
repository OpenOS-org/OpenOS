//! TCP (Transmission Control Protocol) state machine.
//!
//! Implements a minimal TCP stack with connection management, three-way
//! handshake, data transfer, and connection teardown. Runs in kernel space
//! on top of the raw Ethernet frame interface exposed by `drivers::net`.
//!
//! ## Architecture
//!
//! ```text
//! net::handle_frame()  <-- parses IPv4, protocol 6
//!     |
//!     +-- TCP: tcp::handle_tcp_packet() --> connection table lookup
//!                                            state machine transitions
//!                                            ACK generation
//! ```
//!
//! ## TCP Header Format (RFC 793)
//!
//! ```text
//!  0                   1                   2                   3
//!  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |          Source Port          |       Destination Port        |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                        Sequence Number                        |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                    Acknowledgment Number                      |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |  Data |       |C|E|U|A|P|R|S|F|                               |
//! | Offset| Rsrvd |W|C|R|C|S|S|Y|I|            Window             |
//! |       |       |R|E|G|K|H|T|N|N|                               |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |           Checksum            |         Urgent Pointer        |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! ```
//!
//! ## Connection State Machine
//!
//! ```text
//!   Closed ──SYN──> SynSent ──SYN-ACK──> Established
//!   Established ──FIN──> FinWait1 ──ACK──> FinWait2 ──FIN──> TimeWait
//!   Established ──FIN──> CloseWait ──FIN──> LastAck ──ACK──> Closed
//! ```

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

use spin::Mutex;

use crate::serial_println;

// ─────────────────── TCP constants ───────────────────

/// TCP header minimum size (20 bytes, no options).
const TCP_HEADER_MIN_SIZE: usize = 20;

/// TCP data offset for a 20-byte header (5 x 32-bit words).
const TCP_DATA_OFFSET_5: u8 = 5;

/// Default TCP window size (bytes).
const DEFAULT_WINDOW_SIZE: u16 = 65535;

/// Maximum segment size (MTU 1500 - IP header 20 - TCP header 20).
const MAX_SEGMENT_SIZE: usize = 1460;

/// Initial sequence number for new connections.
const INITIAL_SEQ: u32 = 1000;

/// Retransmission timeout in timer ticks (~18.2 Hz, so ~18 ticks = 1 second).
const RETRANSMIT_TIMEOUT_TICKS: u64 = 18;

/// Keepalive interval in timer ticks (~18.2 Hz, so ~1092 ticks = 60 seconds).
const KEEPALIVE_INTERVAL_TICKS: u64 = 1092;

/// Maximum retransmission attempts before giving up.
const MAX_RETRANSMIT_ATTEMPTS: u32 = 5;

/// IP protocol number for TCP (RFC 793).
pub const IP_PROTO_TCP: u8 = 6;

/// IPv4 header minimum size (20 bytes).
const IP_HEADER_MIN_SIZE: usize = 20;

/// Ethernet header size (14 bytes).
const ETHERNET_HEADER_SIZE: usize = 14;

/// `EtherType` for IPv4 (0x0800).
const ETHERTYPE_IPV4: u16 = 0x0800;

/// Default IPv4 TTL.
const IP_DEFAULT_TTL: u8 = 64;

/// IPv4 version (4) in high nibble.
const IP_VERSION_4: u8 = 4;

/// IPv4 IHL for 20-byte header (no options).
const IP_IHL_NO_OPTIONS: u8 = 5;

// ─────────────────── TCP flags ───────────────────

/// FIN flag (bit 0 of the flags byte).
const TCP_FLAG_FIN: u8 = 0x01;

/// SYN flag (bit 1).
const TCP_FLAG_SYN: u8 = 0x02;

/// RST flag (bit 2).
#[allow(dead_code)]
const TCP_FLAG_RST: u8 = 0x04;

/// PSH flag (bit 3).
const TCP_FLAG_PSH: u8 = 0x08;

/// ACK flag (bit 4).
const TCP_FLAG_ACK: u8 = 0x10;

// ─────────────────── Connection states ───────────────────

/// TCP connection states (RFC 793).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpState {
    /// No connection.
    Closed,
    /// SYN sent, waiting for SYN-ACK.
    SynSent,
    /// SYN received, waiting for ACK.
    SynReceived,
    /// Connection established, data transfer active.
    Established,
    /// FIN sent, waiting for ACK.
    FinWait1,
    /// ACK of FIN received, waiting for peer's FIN.
    FinWait2,
    /// FIN received from peer, waiting for application to close.
    CloseWait,
    /// FIN sent after `CloseWait`, waiting for ACK.
    LastAck,
    /// 2MSL wait after connection close.
    TimeWait,
    /// Simultaneous close: both sides sent FIN.
    Closing,
}

// ─────────────────── TCP header ───────────────────

/// Parsed TCP header.
#[derive(Debug, Clone, Copy)]
pub struct TcpHeader {
    /// Source port number.
    pub src_port: u16,
    /// Destination port number.
    pub dst_port: u16,
    /// Sequence number.
    pub seq: u32,
    /// Acknowledgment number.
    pub ack: u32,
    /// Flags byte (SYN, ACK, FIN, etc.).
    pub flags: u8,
    /// Receive window size.
    pub window: u16,
    /// Header checksum.
    pub checksum: u16,
    /// Urgent pointer.
    pub urgent: u16,
    /// Data offset in bytes (header length).
    pub data_offset: usize,
}

// ─────────────────── Retransmit queue entry ───────────────────

/// A segment queued for potential retransmission.
#[derive(Debug, Clone)]
struct RetransmitEntry {
    /// Sequence number of the segment.
    seq: u32,
    /// The raw TCP segment bytes (header + payload).
    data: Vec<u8>,
    /// Remote IP address (network byte order).
    remote_addr: u32,
    /// Timer tick when this segment was last sent.
    sent_at: u64,
    /// Number of retransmission attempts for this segment.
    attempts: u32,
}

// ─────────────────── TCP connection ───────────────────

/// Represents a single TCP connection.
#[derive(Debug)]
pub struct TcpConnection {
    /// Current state of the connection.
    pub state: TcpState,
    /// Local port number.
    pub local_port: u16,
    /// Remote IPv4 address (network byte order).
    pub remote_addr: u32,
    /// Remote port number.
    pub remote_port: u16,
    /// Next sequence number to send.
    pub seq_num: u32,
    /// Next expected sequence number from peer.
    pub ack_num: u32,
    /// Receive window size (bytes).
    pub recv_window: u16,
    /// Send window size (bytes, advertised by peer).
    pub send_window: u16,
    /// Queue of segments awaiting acknowledgment.
    retransmit_queue: Vec<RetransmitEntry>,
    /// Reassembled incoming data buffer.
    recv_buffer: Vec<u8>,
    /// Congestion window size in bytes (slow start: starts at 1 MSS).
    cwnd: usize,
    /// Slow start threshold in bytes.
    ssthresh: usize,
    /// Timer tick of the last received segment (for keepalive).
    last_recv_tick: u64,
    /// Timer tick of the last sent segment (for keepalive).
    last_send_tick: u64,
}

// ─────────────────── Connection table ───────────────────

/// Global TCP connection table.
///
/// Key: (`local_port`, `remote_ip`, `remote_port`) -- uniquely identifies a
/// connection endpoint.
static CONNECTIONS: Mutex<BTreeMap<(u16, u32, u16), TcpConnection>> = Mutex::new(BTreeMap::new());

// ─────────────────── Listen / Accept tables ───────────────────

/// Global listen socket registry.
///
/// Maps `local_port` to a pending-accept queue for that port. When a SYN
/// arrives for a port with a listening socket, a new child `TcpConnection`
/// is created and pushed onto the queue. `sys_accept` pops from the front.
static PENDING_ACCEPT: Mutex<BTreeMap<u16, alloc::vec::Vec<(u32, u16)>>> =
    Mutex::new(BTreeMap::new());

/// Register a port as a listening socket (passive open).
///
/// Called from `sys_listen`. Stores the port in the listen table so that
/// incoming SYN segments can be matched against it.
pub fn register_listen_port(local_port: u16) {
    let mut pending = PENDING_ACCEPT.lock();
    pending.entry(local_port).or_default();
    serial_println!("[TCP] Listening on port {}", local_port);
}

/// Remove a port from the listen registry.
///
/// Called when a listening socket is closed.
pub fn unregister_listen_port(local_port: u16) {
    let mut pending = PENDING_ACCEPT.lock();
    pending.remove(&local_port);
    serial_println!("[TCP] Stopped listening on port {}", local_port);
}

/// Pop the next accepted connection from a listening port's queue.
///
/// Returns `Some((remote_addr, remote_port))` if a connection is pending,
/// or `None` if the queue is empty.
pub fn accept_next(local_port: u16) -> Option<(u32, u16)> {
    let mut pending = PENDING_ACCEPT.lock();
    if let Some(queue) = pending.get_mut(&local_port) {
        if !queue.is_empty() {
            return Some(queue.remove(0));
        }
    }
    None
}

/// Check if a port has pending accept connections (non-blocking).
pub fn has_pending_accept(local_port: u16) -> bool {
    let pending = PENDING_ACCEPT.lock();
    pending.get(&local_port).is_some_and(|q| !q.is_empty())
}

// ─────────────────── Byte helpers ───────────────────

/// Read a big-endian `u16` from `data[offset..]`.
fn read_u16_be(data: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([data[offset], data[offset + 1]])
}

/// Read a big-endian `u32` from `data[offset..]`.
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

// ─────────────────── Checksum ───────────────────

/// Compute the TCP checksum with a pseudo-header (RFC 793).
///
/// The pseudo-header includes source IP, destination IP, protocol, and
/// TCP segment length.
fn tcp_checksum(src_ip: u32, dst_ip: u32, tcp_segment: &[u8]) -> u16 {
    let tcp_len = tcp_segment.len();

    // Build pseudo-header (12 bytes) + TCP segment.
    let mut buf = Vec::with_capacity(12 + tcp_len);
    buf.extend_from_slice(&src_ip.to_be_bytes());
    buf.extend_from_slice(&dst_ip.to_be_bytes());
    buf.push(0); // reserved
    buf.push(IP_PROTO_TCP);
    buf.extend_from_slice(&(tcp_len as u16).to_be_bytes());
    buf.extend_from_slice(tcp_segment);

    internet_checksum(&buf)
}

/// Compute the Internet checksum (RFC 1071).
fn internet_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;

    while i + 1 < data.len() {
        sum += u32::from(u16::from_be_bytes([data[i], data[i + 1]]));
        i += 2;
    }

    if i < data.len() {
        sum += u32::from(data[i]) << 8;
    }

    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    !sum as u16
}

// ─────────────────── TCP header parsing ───────────────────

/// Parse a TCP header from `data`.
///
/// Returns `None` if the data is too short or the data offset is invalid.
#[must_use]
pub fn parse_tcp(data: &[u8]) -> Option<(TcpHeader, &[u8])> {
    if data.len() < TCP_HEADER_MIN_SIZE {
        return None;
    }

    let data_offset = ((data[12] >> 4) & 0x0F) as usize;
    let header_len = data_offset * 4;

    if header_len < TCP_HEADER_MIN_SIZE || data.len() < header_len {
        return None;
    }

    let header = TcpHeader {
        src_port: read_u16_be(data, 0),
        dst_port: read_u16_be(data, 2),
        seq: read_u32_be(data, 4),
        ack: read_u32_be(data, 8),
        flags: data[13],
        window: read_u16_be(data, 14),
        checksum: read_u16_be(data, 16),
        urgent: read_u16_be(data, 18),
        data_offset: header_len,
    };

    Some((header, &data[header_len..]))
}

// ─────────────────── TCP segment building ───────────────────

/// Build a TCP segment (header + payload).
///
/// The checksum field is computed over the pseudo-header + segment.
/// Returns a complete TCP segment ready for IPv4 encapsulation.
#[must_use]
pub fn build_tcp(
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    window: u16,
    payload: &[u8],
) -> Vec<u8> {
    let header_len = TCP_HEADER_MIN_SIZE;
    let total_len = header_len + payload.len();
    let mut segment = vec![0u8; total_len];

    // Source port.
    write_u16_be(&mut segment, 0, src_port);
    // Destination port.
    write_u16_be(&mut segment, 2, dst_port);
    // Sequence number.
    write_u32_be(&mut segment, 4, seq);
    // Acknowledgment number.
    write_u32_be(&mut segment, 8, ack);
    // Data offset (5 = 20 bytes) and reserved bits.
    segment[12] = TCP_DATA_OFFSET_5 << 4;
    // Flags.
    segment[13] = flags;
    // Window.
    write_u16_be(&mut segment, 14, window);
    // Checksum (field at 16..18) -- computed below.
    // Urgent pointer.
    write_u16_be(&mut segment, 18, 0);

    // Copy payload.
    segment[header_len..].copy_from_slice(payload);

    segment
}

// ─────────────────── IPv4 packet building ───────────────────

/// Build an IPv4 packet containing a TCP segment.
///
/// Computes the IPv4 header checksum and wraps the TCP segment.
fn build_tcp_ip_packet(src_ip: u32, dst_ip: u32, tcp_segment: &[u8]) -> Vec<u8> {
    let ip_total_len = (IP_HEADER_MIN_SIZE + tcp_segment.len()) as u16;

    let mut ip_header = vec![0u8; IP_HEADER_MIN_SIZE];
    ip_header[0] = (IP_VERSION_4 << 4) | IP_IHL_NO_OPTIONS;
    write_u16_be(&mut ip_header, 2, ip_total_len);
    // Identification: 0.
    // Flags: Don't Fragment.
    ip_header[6] = 0x40;
    ip_header[8] = IP_DEFAULT_TTL;
    ip_header[9] = IP_PROTO_TCP;
    write_u32_be(&mut ip_header, 12, src_ip);
    write_u32_be(&mut ip_header, 16, dst_ip);

    // Compute and set IPv4 header checksum.
    let ip_checksum = super::internet_checksum(&ip_header);
    write_u16_be(&mut ip_header, 10, ip_checksum);

    // Now compute the TCP checksum with the pseudo-header.
    let mut tcp_segment_mut = tcp_segment.to_vec();
    let checksum = tcp_checksum(src_ip, dst_ip, &tcp_segment_mut);
    write_u16_be(&mut tcp_segment_mut, 16, checksum);

    // Assemble: IP header + TCP segment.
    let mut packet = Vec::with_capacity(IP_HEADER_MIN_SIZE + tcp_segment_mut.len());
    packet.extend_from_slice(&ip_header);
    packet.extend_from_slice(&tcp_segment_mut);
    packet
}

/// Build an Ethernet frame containing a TCP/IP packet.
fn build_tcp_frame(dst_mac: [u8; 6], src_ip: u32, dst_ip: u32, tcp_segment: &[u8]) -> Vec<u8> {
    let ip_packet = build_tcp_ip_packet(src_ip, dst_ip, tcp_segment);
    super::build_ethernet(
        dst_mac,
        crate::drivers::net::mac_address(),
        ETHERTYPE_IPV4,
        &ip_packet,
    )
}

// ─────────────────── Packet sending ───────────────────

/// Send a TCP segment to the remote peer.
///
/// Resolves the remote MAC via ARP, builds the Ethernet frame, and
/// transmits it via the network driver.
#[allow(clippy::too_many_arguments)]
fn send_tcp_segment(
    remote_addr: u32,
    local_port: u16,
    remote_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    window: u16,
    payload: &[u8],
) {
    let segment = build_tcp(local_port, remote_port, seq, ack, flags, window, payload);
    let local_ip = super::local_ip();

    // Look up the remote MAC via ARP. If not found, send an ARP request
    // and drop this segment (the retransmit logic will retry).
    let Some(dst_mac) = super::arp_lookup(remote_addr) else {
        serial_println!(
            "[TCP] No ARP entry for {:?}, sending ARP request",
            super::FormatIp(remote_addr)
        );
        super::send_arp_request(remote_addr);
        return;
    };

    let frame = build_tcp_frame(dst_mac, local_ip, remote_addr, &segment);

    match crate::drivers::net::send_frame(&frame) {
        Ok(sent) => {
            serial_println!(
                "[TCP] Sent segment: {}:{} -> {:?}:{} flags={:#04x} seq={} ack={} len={}",
                local_port,
                local_port,
                super::FormatIp(remote_addr),
                remote_port,
                flags,
                seq,
                ack,
                payload.len()
            );
        }
        Err(e) => {
            serial_println!("[TCP] Send failed: {:?}", e);
        }
    }
}

// ─────────────────── Connection management ───────────────────

/// Create a new TCP connection entry.
///
/// Inserts a connection in the `Closed` state into the connection table.
///
/// # Errors
///
/// Returns `Err(())` if a connection with the same key already exists.
pub fn new_connection(local_port: u16, remote_addr: u32, remote_port: u16) -> Result<(), ()> {
    let key = (local_port, remote_addr, remote_port);
    let mut conns = CONNECTIONS.lock();

    if conns.contains_key(&key) {
        return Err(());
    }

    let now = crate::arch::x86_64::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);
    let conn = TcpConnection {
        state: TcpState::Closed,
        local_port,
        remote_addr,
        remote_port,
        seq_num: INITIAL_SEQ,
        ack_num: 0,
        recv_window: DEFAULT_WINDOW_SIZE,
        send_window: DEFAULT_WINDOW_SIZE,
        retransmit_queue: Vec::new(),
        recv_buffer: Vec::new(),
        cwnd: MAX_SEGMENT_SIZE,
        ssthresh: (DEFAULT_WINDOW_SIZE as usize).saturating_mul(2),
        last_recv_tick: now,
        last_send_tick: now,
    };

    conns.insert(key, conn);
    serial_println!(
        "[TCP] New connection: {} -> {:?}:{}",
        local_port,
        super::FormatIp(remote_addr),
        remote_port
    );
    Ok(())
}

/// Allocate a local port for a new connection.
///
/// Scans ports starting from 49152 (ephemeral range) to find a free one.
/// Returns `None` if no free port is available.
pub fn allocate_local_port() -> Option<u16> {
    /// Start of the ephemeral port range (IANA).
    const EPHEMERAL_START: u16 = 49152;

    let conns = CONNECTIONS.lock();
    let mut port = EPHEMERAL_START;

    loop {
        // Check if any connection uses this local port.
        let in_use = conns.keys().any(|&(lp, _, _)| lp == port);
        if !in_use {
            return Some(port);
        }
        if port == u16::MAX {
            break;
        }
        port += 1;
    }

    None
}

// ─────────────────── Three-way handshake ───────────────────

/// Initiate a TCP connection (active open).
///
/// Performs the three-way handshake:
/// 1. Send SYN (state: `Closed` -> `SynSent`)
/// 2. Receive SYN-ACK (handled by `handle_tcp_packet`)
/// 3. Send ACK (state: `SynSent` -> `Established`)
///
/// Note: This function sends the SYN and transitions to `SynSent`.
/// The SYN-ACK handling is done asynchronously in `handle_tcp_packet`.
///
/// # Errors
///
/// Returns `Err(())` if the connection already exists or the connection
/// entry cannot be created.
pub fn connect(local_port: u16, remote_addr: u32, remote_port: u16) -> Result<(), ()> {
    // Create the connection entry.
    new_connection(local_port, remote_addr, remote_port)?;

    let key = (local_port, remote_addr, remote_port);
    let mut conns = CONNECTIONS.lock();
    let conn = conns.get_mut(&key).ok_or(())?;

    // Transition to SynSent.
    conn.state = TcpState::SynSent;

    let seq = conn.seq_num;
    let window = conn.recv_window;

    serial_println!(
        "[TCP] Connecting: {} -> {:?}:{} (SYN seq={})",
        local_port,
        super::FormatIp(remote_addr),
        remote_port,
        seq
    );

    drop(conns);

    // Send SYN.
    send_tcp_segment(
        remote_addr,
        local_port,
        remote_port,
        seq,
        0,
        TCP_FLAG_SYN,
        window,
        &[],
    );

    // Queue SYN for retransmission.
    let syn_segment = build_tcp(local_port, remote_port, seq, 0, TCP_FLAG_SYN, window, &[]);
    let mut conns = CONNECTIONS.lock();
    if let Some(conn) = conns.get_mut(&key) {
        let now =
            crate::arch::x86_64::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);
        conn.retransmit_queue.push(RetransmitEntry {
            seq,
            data: syn_segment,
            remote_addr,
            sent_at: now,
            attempts: 0,
        });
    }

    Ok(())
}

// ─────────────────── Data transfer ───────────────────

/// Send data over an established TCP connection.
///
/// Segments the data into MSS-sized chunks and sends each segment
/// with the PSH+ACK flags. Updates the sequence number.
///
/// # Errors
///
/// Returns `Err(())` if the connection does not exist or is not established.
pub fn send_data(
    local_port: u16,
    remote_addr: u32,
    remote_port: u16,
    data: &[u8],
) -> Result<usize, ()> {
    let key = (local_port, remote_addr, remote_port);
    let mut conns = CONNECTIONS.lock();
    let conn = conns.get_mut(&key).ok_or(())?;

    if conn.state != TcpState::Established {
        serial_println!("[TCP] send_data: connection not established");
        return Err(());
    }

    let mut total_sent = 0;
    let local_ip = super::local_ip();

    while total_sent < data.len() {
        // Respect both the congestion window and the peer's send window.
        let cwnd_limit = conn.cwnd.min(conn.send_window as usize);
        let remaining = data.len() - total_sent;
        let chunk_size = remaining.min(MAX_SEGMENT_SIZE).min(cwnd_limit);
        let chunk = &data[total_sent..total_sent + chunk_size];

        let seq = conn.seq_num;
        let ack = conn.ack_num;
        let window = conn.recv_window;

        // Build and send the segment.
        let segment = build_tcp(
            local_port,
            remote_port,
            seq,
            ack,
            TCP_FLAG_PSH | TCP_FLAG_ACK,
            window,
            chunk,
        );

        let Some(dst_mac) = super::arp_lookup(remote_addr) else {
            serial_println!("[TCP] No ARP for send_data, dropping");
            break;
        };

        let frame = build_tcp_frame(dst_mac, local_ip, remote_addr, &segment);

        match crate::drivers::net::send_frame(&frame) {
            Ok(_) => {
                serial_println!("[TCP] Sent data: seq={} len={}", seq, chunk_size);
            }
            Err(e) => {
                serial_println!("[TCP] Data send failed: {:?}", e);
                break;
            }
        }

        // Queue for retransmission.
        let now =
            crate::arch::x86_64::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);
        conn.retransmit_queue.push(RetransmitEntry {
            seq,
            data: segment,
            remote_addr,
            sent_at: now,
            attempts: 0,
        });
        conn.last_send_tick = now;

        // Advance sequence number.
        conn.seq_num = seq.wrapping_add(chunk_size as u32);
        total_sent += chunk_size;
    }

    Ok(total_sent)
}

/// Receive data from a TCP connection (non-blocking).
///
/// Copies available data from the receive buffer into `buf`.
/// Returns the number of bytes copied.
///
/// # Errors
///
/// Returns `Err(())` if the connection does not exist or is not in a
/// data transfer state.
pub fn recv_data(
    local_port: u16,
    remote_addr: u32,
    remote_port: u16,
    buf: &mut [u8],
) -> Result<usize, ()> {
    let key = (local_port, remote_addr, remote_port);
    let mut conns = CONNECTIONS.lock();
    let conn = conns.get_mut(&key).ok_or(())?;

    if conn.state != TcpState::Established && conn.state != TcpState::CloseWait {
        serial_println!("[TCP] recv_data: connection not in data transfer state");
        return Err(());
    }

    let available = conn.recv_buffer.len();
    if available == 0 {
        return Ok(0);
    }

    let to_copy = available.min(buf.len());
    buf[..to_copy].copy_from_slice(&conn.recv_buffer[..to_copy]);

    // Remove copied data from the buffer.
    conn.recv_buffer.drain(..to_copy);

    Ok(to_copy)
}

// ─────────────────── Connection teardown ───────────────────

/// Close a TCP connection.
///
/// Initiates the four-way handshake (simplified to three):
/// 1. Send FIN (state transitions based on current state)
/// 2. Receive FIN-ACK (handled by `handle_tcp_packet`)
/// 3. Connection reaches `Closed` or `TimeWait`
///
/// # Errors
///
/// Returns `Err(())` if the connection does not exist or is in an invalid
/// state for closing.
pub fn close(local_port: u16, remote_addr: u32, remote_port: u16) -> Result<(), ()> {
    let key = (local_port, remote_addr, remote_port);
    let mut conns = CONNECTIONS.lock();
    let conn = conns.get_mut(&key).ok_or(())?;

    match conn.state {
        TcpState::Established => {
            conn.state = TcpState::FinWait1;
        }
        TcpState::CloseWait => {
            conn.state = TcpState::LastAck;
        }
        _ => {
            serial_println!("[TCP] close: invalid state {:?}", conn.state);
            return Err(());
        }
    }

    let seq = conn.seq_num;
    let ack = conn.ack_num;
    let window = conn.recv_window;
    let new_state = conn.state;

    drop(conns);

    serial_println!(
        "[TCP] Closing: {} -> {:?}:{} (state -> {:?})",
        local_port,
        super::FormatIp(remote_addr),
        remote_port,
        new_state
    );

    // Send FIN+ACK.
    send_tcp_segment(
        remote_addr,
        local_port,
        remote_port,
        seq,
        ack,
        TCP_FLAG_FIN | TCP_FLAG_ACK,
        window,
        &[],
    );

    // Advance seq for the FIN.
    let mut conns = CONNECTIONS.lock();
    if let Some(conn) = conns.get_mut(&key) {
        conn.seq_num = seq.wrapping_add(1);
    }

    Ok(())
}

// ─────────────────── Incoming packet handler ───────────────────

/// Handle an incoming TCP packet.
///
/// Called from `net::handle_frame` when an IPv4 packet with protocol 6
/// arrives. Looks up the connection by (`dst_port`, `src_ip`, `src_port`)
/// and processes the segment through the state machine.
pub fn handle_tcp_packet(src_ip: u32, dst_ip: u32, tcp_data: &[u8]) {
    let Some((header, payload)) = parse_tcp(tcp_data) else {
        serial_println!("[TCP] Parse failed, dropping");
        return;
    };

    // The packet's destination is us, so dst_port is our local port.
    let local_port = header.dst_port;
    let remote_port = header.src_port;
    let key = (local_port, src_ip, remote_port);

    serial_println!(
        "[TCP] Received: {:?}:{} -> {} flags={:#04x} seq={} ack={} len={}",
        super::FormatIp(src_ip),
        remote_port,
        local_port,
        header.flags,
        header.seq,
        header.ack,
        payload.len()
    );

    let mut conns = CONNECTIONS.lock();

    // Look up the connection.
    let Some(conn) = conns.get_mut(&key) else {
        // No exact match found. Check for wildcard listen socket on SYN.
        if header.flags & TCP_FLAG_SYN != 0 && header.flags & TCP_FLAG_ACK == 0 {
            // Check if there's a listening socket on the destination port.
            let is_listening = {
                let pending = PENDING_ACCEPT.lock();
                pending.contains_key(&local_port)
            };

            if is_listening {
                serial_println!(
                    "[TCP] SYN on listening port {} from {:?}:{}, creating child connection",
                    local_port,
                    super::FormatIp(src_ip),
                    remote_port
                );
                drop(conns);
                handle_passive_syn(src_ip, local_port, remote_port, &header);
                return;
            }
        }

        // No matching connection -- send RST if ACK is not set, otherwise drop.
        if header.flags & TCP_FLAG_ACK == 0 {
            serial_println!("[TCP] No connection for port {}, sending RST", local_port);
            drop(conns);
            send_tcp_segment(
                src_ip,
                local_port,
                remote_port,
                0,
                header
                    .seq
                    .wrapping_add(payload.len() as u32)
                    .wrapping_add(u32::from(header.flags & TCP_FLAG_SYN != 0)),
                TCP_FLAG_RST | TCP_FLAG_ACK,
                0,
                &[],
            );
        }
        return;
    };

    match conn.state {
        TcpState::SynSent => handle_syn_sent(conn, &header, src_ip, payload),
        TcpState::SynReceived => handle_syn_received(conn, &header, src_ip, payload),
        TcpState::Established => handle_established(conn, &header, src_ip, payload),
        TcpState::FinWait1 => handle_fin_wait1(conn, &header, src_ip, payload),
        TcpState::FinWait2 => handle_fin_wait2(conn, &header, src_ip, payload),
        TcpState::CloseWait => handle_close_wait(conn, &header, src_ip, payload),
        TcpState::LastAck => handle_last_ack(conn, &header, src_ip, payload),
        TcpState::TimeWait => {
            // In TimeWait, respond to any segments with ACK.
            send_tcp_segment(
                src_ip,
                conn.local_port,
                conn.remote_port,
                conn.seq_num,
                conn.ack_num,
                TCP_FLAG_ACK,
                conn.recv_window,
                &[],
            );
        }
        TcpState::Closing => handle_closing(conn, &header, src_ip, payload),
        TcpState::Closed => {
            // Ignore segments for closed connections.
        }
    }
}

// ─────────────────── Passive open (SYN handling) ───────────────────

/// Handle a SYN arriving on a listening port (passive open).
///
/// Creates a new child connection in `SynReceived` state, sends SYN-ACK,
/// and queues it for `sys_accept`.
fn handle_passive_syn(src_ip: u32, local_port: u16, remote_port: u16, header: &TcpHeader) {
    // Create a new connection for this client.
    let result = new_connection(local_port, src_ip, remote_port);
    if result.is_err() {
        serial_println!(
            "[TCP] passive open: connection already exists for {} -> {:?}:{}",
            local_port,
            super::FormatIp(src_ip),
            remote_port
        );
        return;
    }

    let key = (local_port, src_ip, remote_port);
    let mut conns = CONNECTIONS.lock();
    let Some(conn) = conns.get_mut(&key) else {
        return;
    };

    // Set up the SYN-ACK response.
    let server_seq = INITIAL_SEQ;
    conn.seq_num = server_seq.wrapping_add(1); // +1 for the SYN consuming a seq
    conn.ack_num = header.seq.wrapping_add(1); // ACK the client's SYN
    conn.state = TcpState::SynReceived;

    serial_println!(
        "[TCP] Passive open: {} <- {:?}:{}, sending SYN-ACK (seq={}, ack={})",
        local_port,
        super::FormatIp(src_ip),
        remote_port,
        server_seq,
        conn.ack_num
    );

    // Build SYN-ACK segment for retransmit queue.
    let syn_ack_segment = build_tcp(
        local_port,
        remote_port,
        server_seq,
        conn.ack_num,
        TCP_FLAG_SYN | TCP_FLAG_ACK,
        conn.recv_window,
        &[],
    );

    let now = crate::arch::x86_64::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);
    conn.retransmit_queue.push(RetransmitEntry {
        seq: server_seq,
        data: syn_ack_segment,
        remote_addr: src_ip,
        sent_at: now,
        attempts: 0,
    });
    conn.last_recv_tick = now;
    conn.last_send_tick = now;

    // Collect info before dropping the lock.
    let window = conn.recv_window;

    drop(conns);

    // Send SYN-ACK.
    send_tcp_segment(
        src_ip,
        local_port,
        remote_port,
        server_seq,
        header.seq.wrapping_add(1),
        TCP_FLAG_SYN | TCP_FLAG_ACK,
        window,
        &[],
    );

    // Enqueue this connection for accept.
    let mut pending = PENDING_ACCEPT.lock();
    if let Some(queue) = pending.get_mut(&local_port) {
        queue.push((src_ip, remote_port));
        serial_println!(
            "[TCP] Enqueued accepted connection: {:?}:{} (queue len={})",
            super::FormatIp(src_ip),
            remote_port,
            queue.len()
        );
    }
}

// ─────────────────── State handlers ───────────────────

/// Handle a segment in the `SynSent` state.
///
/// Expects a SYN-ACK from the server. On receipt, sends ACK and
/// transitions to `Established`.
fn handle_syn_sent(conn: &mut TcpConnection, header: &TcpHeader, src_ip: u32, payload: &[u8]) {
    // We expect SYN+ACK.
    if header.flags & (TCP_FLAG_SYN | TCP_FLAG_ACK) == (TCP_FLAG_SYN | TCP_FLAG_ACK) {
        // Verify the ACK acknowledges our SYN.
        if header.ack != conn.seq_num.wrapping_add(1) {
            serial_println!(
                "[TCP] SynSent: bad ACK (expected {}, got {})",
                conn.seq_num.wrapping_add(1),
                header.ack
            );
            return;
        }

        // Update connection state.
        conn.ack_num = header.seq.wrapping_add(1);
        conn.seq_num = header.ack;
        conn.send_window = header.window;
        conn.state = TcpState::Established;

        // Remove SYN from retransmit queue.
        conn.retransmit_queue.clear();

        serial_println!(
            "[TCP] Established: {} -> {:?}:{}",
            conn.local_port,
            super::FormatIp(conn.remote_addr),
            conn.remote_port
        );

        // Send ACK to complete the handshake.
        drop_retransmit_and_send_ack(conn, src_ip);

        // Process any data piggy-backed on the SYN-ACK.
        if !payload.is_empty() {
            conn.recv_buffer.extend_from_slice(payload);
        }
    } else if header.flags & TCP_FLAG_RST != 0 {
        serial_println!("[TCP] SynSent: RST received");
        conn.state = TcpState::Closed;
    }
}

/// Handle a segment in the `SynReceived` state.
///
/// Expects an ACK to complete the handshake.
fn handle_syn_received(conn: &mut TcpConnection, header: &TcpHeader, _src_ip: u32, payload: &[u8]) {
    if header.flags & TCP_FLAG_ACK != 0 {
        if header.ack == conn.seq_num {
            conn.state = TcpState::Established;
            conn.send_window = header.window;

            // Remove SYN-ACK from retransmit queue.
            conn.retransmit_queue.clear();

            serial_println!(
                "[TCP] Established (passive): {} -> {:?}:{}",
                conn.local_port,
                super::FormatIp(conn.remote_addr),
                conn.remote_port
            );

            if !payload.is_empty() {
                conn.recv_buffer.extend_from_slice(payload);
                conn.ack_num = conn.ack_num.wrapping_add(payload.len() as u32);
            }
        }
    } else if header.flags & TCP_FLAG_RST != 0 {
        conn.state = TcpState::Closed;
    }
}

/// Handle a segment in the `Established` state.
///
/// Processes incoming data (ACKs payload, buffers data) and handles
/// connection close (FIN).
fn handle_established(conn: &mut TcpConnection, header: &TcpHeader, src_ip: u32, payload: &[u8]) {
    // Handle RST.
    if header.flags & TCP_FLAG_RST != 0 {
        serial_println!("[TCP] Established: RST received");
        conn.state = TcpState::Closed;
        return;
    }

    // Update last receive tick (for keepalive tracking).
    conn.last_recv_tick =
        crate::arch::x86_64::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);

    // Update send window from peer's advertised window.
    conn.send_window = header.window;

    // Process ACK: remove acknowledged segments from retransmit queue.
    if header.flags & TCP_FLAG_ACK != 0 {
        process_ack(conn, header.ack);
    }

    // Handle FIN.
    if header.flags & TCP_FLAG_FIN != 0 {
        // Peer is closing.
        conn.ack_num = header.seq.wrapping_add(1);
        conn.state = TcpState::CloseWait;

        // Send ACK for the FIN.
        send_tcp_segment(
            src_ip,
            conn.local_port,
            conn.remote_port,
            conn.seq_num,
            conn.ack_num,
            TCP_FLAG_ACK,
            conn.recv_window,
            &[],
        );

        serial_println!(
            "[TCP] CloseWait: {} -> {:?}:{}",
            conn.local_port,
            super::FormatIp(conn.remote_addr),
            conn.remote_port
        );
        return;
    }

    // Process data.
    if !payload.is_empty() {
        conn.recv_buffer.extend_from_slice(payload);
        conn.ack_num = conn.ack_num.wrapping_add(payload.len() as u32);

        // Send ACK for received data.
        send_tcp_segment(
            src_ip,
            conn.local_port,
            conn.remote_port,
            conn.seq_num,
            conn.ack_num,
            TCP_FLAG_ACK,
            conn.recv_window,
            &[],
        );

        serial_println!(
            "[TCP] Received {} bytes, buffered (total {})",
            payload.len(),
            conn.recv_buffer.len()
        );
    }
}

/// Handle a segment in the `FinWait1` state.
///
/// Possible transitions:
/// - ACK of our FIN -> `FinWait2`
/// - FIN (simultaneous close) -> `Closing`
/// - FIN+ACK -> `TimeWait`
fn handle_fin_wait1(conn: &mut TcpConnection, header: &TcpHeader, src_ip: u32, payload: &[u8]) {
    if header.flags & TCP_FLAG_RST != 0 {
        conn.state = TcpState::Closed;
        return;
    }

    if header.flags & TCP_FLAG_ACK != 0 {
        process_ack(conn, header.ack);
    }

    if header.flags & TCP_FLAG_FIN != 0 {
        // Peer also sent FIN.
        conn.ack_num = header.seq.wrapping_add(1);

        if header.flags & TCP_FLAG_ACK != 0 {
            // FIN+ACK: go directly to TimeWait.
            conn.state = TcpState::TimeWait;
            serial_println!("[TCP] TimeWait: {:?}", super::FormatIp(conn.remote_addr));
        } else {
            // FIN only: simultaneous close -> Closing.
            conn.state = TcpState::Closing;
        }

        // ACK the FIN.
        send_tcp_segment(
            src_ip,
            conn.local_port,
            conn.remote_port,
            conn.seq_num,
            conn.ack_num,
            TCP_FLAG_ACK,
            conn.recv_window,
            &[],
        );
    } else if header.flags & TCP_FLAG_ACK != 0 {
        // ACK without FIN: `FinWait1` -> `FinWait2`.
        conn.state = TcpState::FinWait2;
        conn.retransmit_queue.clear();
    }
}

/// Handle a segment in the `FinWait2` state.
///
/// Expects a FIN from the peer to complete the close.
fn handle_fin_wait2(conn: &mut TcpConnection, header: &TcpHeader, src_ip: u32, _payload: &[u8]) {
    if header.flags & TCP_FLAG_RST != 0 {
        conn.state = TcpState::Closed;
        return;
    }

    if header.flags & TCP_FLAG_FIN != 0 {
        conn.ack_num = header.seq.wrapping_add(1);
        conn.state = TcpState::TimeWait;

        // ACK the FIN.
        send_tcp_segment(
            src_ip,
            conn.local_port,
            conn.remote_port,
            conn.seq_num,
            conn.ack_num,
            TCP_FLAG_ACK,
            conn.recv_window,
            &[],
        );

        serial_println!("[TCP] TimeWait: {:?}", super::FormatIp(conn.remote_addr));
    }
}

/// Handle a segment in the `CloseWait` state.
///
/// The application should call `close()` to transition to `LastAck`.
/// We simply ACK any incoming data.
fn handle_close_wait(conn: &mut TcpConnection, header: &TcpHeader, _src_ip: u32, _payload: &[u8]) {
    if header.flags & TCP_FLAG_RST != 0 {
        conn.state = TcpState::Closed;
        return;
    }

    // ACK any segments.
    if header.flags & TCP_FLAG_ACK != 0 {
        process_ack(conn, header.ack);
    }
}

/// Handle a segment in the `LastAck` state.
///
/// Expects a final ACK for our FIN.
fn handle_last_ack(conn: &mut TcpConnection, header: &TcpHeader, _src_ip: u32, _payload: &[u8]) {
    if header.flags & TCP_FLAG_ACK != 0 {
        conn.state = TcpState::Closed;
        conn.retransmit_queue.clear();
        serial_println!(
            "[TCP] Closed: {} -> {:?}:{}",
            conn.local_port,
            super::FormatIp(conn.remote_addr),
            conn.remote_port
        );
    }
}

/// Handle a segment in the `Closing` state.
///
/// Expects an ACK for our FIN to transition to `TimeWait`.
fn handle_closing(conn: &mut TcpConnection, header: &TcpHeader, _src_ip: u32, _payload: &[u8]) {
    if header.flags & TCP_FLAG_ACK != 0 {
        conn.state = TcpState::TimeWait;
        conn.retransmit_queue.clear();
        serial_println!(
            "[TCP] TimeWait (closing): {:?}",
            super::FormatIp(conn.remote_addr)
        );
    }
}

// ─────────────────── Helpers ───────────────────

/// Process an ACK: remove acknowledged segments from the retransmit queue.
///
/// Implements congestion control: on each new ACK, the congestion window
/// grows by one MSS (slow start) until it reaches the slow start threshold,
/// then grows linearly (congestion avoidance).
fn process_ack(conn: &mut TcpConnection, ack_num: u32) {
    let before = conn.retransmit_queue.len();
    conn.retransmit_queue
        .retain(|entry| is_seq_before(ack_num, entry.seq));
    let acked = before.saturating_sub(conn.retransmit_queue.len());

    if acked > 0 {
        // Slow start: double cwnd per RTT (grow by MSS per ACK).
        if conn.cwnd < conn.ssthresh {
            conn.cwnd = (conn.cwnd + MAX_SEGMENT_SIZE).min(conn.ssthresh);
        } else {
            // Congestion avoidance: linear growth (add MSS per full window ACK'd).
            conn.cwnd += MAX_SEGMENT_SIZE * MAX_SEGMENT_SIZE / conn.cwnd;
        }
    }
}

/// Send an ACK and clear the retransmit queue for this connection.
fn drop_retransmit_and_send_ack(conn: &TcpConnection, remote_ip: u32) {
    send_tcp_segment(
        remote_ip,
        conn.local_port,
        conn.remote_port,
        conn.seq_num,
        conn.ack_num,
        TCP_FLAG_ACK,
        conn.recv_window,
        &[],
    );
}

/// Check if `seq_a` is before `seq_b` in TCP sequence space (handles wrap).
///
/// Uses wrapping subtraction and reinterprets as signed to handle
/// sequence number wraparound correctly.
#[allow(clippy::cast_possible_wrap)]
fn is_seq_before(seq_a: u32, seq_b: u32) -> bool {
    let diff = seq_a.wrapping_sub(seq_b);
    // SAFETY: Reinterpret the wrapping difference as signed to check ordering.
    // This is the standard TCP sequence comparison algorithm (RFC 793).
    (diff as i32) < 0
}

/// Get the state of a connection (for syscall layer).
pub fn get_connection_state(
    local_port: u16,
    remote_addr: u32,
    remote_port: u16,
) -> Option<TcpState> {
    let key = (local_port, remote_addr, remote_port);
    let conns = CONNECTIONS.lock();
    conns.get(&key).map(|c| c.state)
}

/// Remove a connection from the table.
pub fn remove_connection(local_port: u16, remote_addr: u32, remote_port: u16) -> bool {
    let key = (local_port, remote_addr, remote_port);
    CONNECTIONS.lock().remove(&key).is_some()
}

/// Get the local IP for use by callers.
#[must_use]
pub fn local_ip() -> u32 {
    super::local_ip()
}

/// Summary information about a TCP connection (for `/proc/net/tcp`).
#[derive(Debug, Clone)]
pub struct TcpConnectionInfo {
    /// Local port number.
    pub local_port: u16,
    /// Remote IPv4 address (network byte order).
    pub remote_addr: u32,
    /// Remote port number.
    pub remote_port: u16,
    /// Current connection state.
    pub state: TcpState,
}

/// List all TCP connections.
///
/// Returns a snapshot of all connections in the global table. Used by
/// `/proc/net/tcp` to expose connection state to user-space.
pub fn list_connections() -> alloc::vec::Vec<TcpConnectionInfo> {
    let conns = CONNECTIONS.lock();
    conns
        .iter()
        .map(|((_lp, ra, rp), c)| TcpConnectionInfo {
            local_port: c.local_port,
            remote_addr: *ra,
            remote_port: *rp,
            state: c.state,
        })
        .collect()
}

// ─────────────────── Periodic maintenance ───────────────────

/// `TIME_WAIT` duration in ticks (`2MSL` = 60 seconds at ~18.2 Hz).
const TIME_WAIT_DURATION_TICKS: u64 = 1_092;

/// Check all TCP connections for timeouts and cleanup.
///
/// Should be called regularly from the network service loop or timer tick.
/// Checks all connections for:
/// - Segments that have exceeded the retransmission timeout
/// - Idle connections that need keepalive probes
/// - `TIME_WAIT` connections that have exceeded `2MSL`
pub fn periodic_tick() {
    let now = crate::arch::x86_64::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);
    let mut conns = CONNECTIONS.lock();

    // ── TIME_WAIT cleanup ──
    // Remove connections that have been in TIME_WAIT for longer than 2MSL.
    let tw_keys: Vec<(u16, u32, u16)> = conns
        .iter()
        .filter(|(_, c)| c.state == TcpState::TimeWait)
        .map(|(k, _)| *k)
        .collect();
    for key in tw_keys {
        if let Some(conn) = conns.get(&key) {
            let idle = now.saturating_sub(conn.last_recv_tick);
            if idle >= TIME_WAIT_DURATION_TICKS {
                conns.remove(&key);
                serial_println!(
                    "[TCP] TIME_WAIT expired: {:?}:{}, removed",
                    super::FormatIp(key.1),
                    key.2
                );
            }
        }
    }

    // Collect keys of established connections to avoid borrow issues.
    let keys: Vec<(u16, u32, u16)> = conns
        .iter()
        .filter(|(_, c)| c.state == TcpState::Established)
        .map(|(k, _)| *k)
        .collect();

    for key in keys {
        let Some(conn) = conns.get_mut(&key) else {
            continue;
        };

        // ── Retransmission check ──
        let mut retransmit_segments: Vec<(u32, Vec<u8>, u32)> = Vec::new();
        for entry in &mut conn.retransmit_queue {
            if now.saturating_sub(entry.sent_at) >= RETRANSMIT_TIMEOUT_TICKS
                && entry.attempts < MAX_RETRANSMIT_ATTEMPTS
            {
                retransmit_segments.push((entry.seq, entry.data.clone(), entry.remote_addr));
                entry.sent_at = now;
                entry.attempts += 1;
                // Halve cwnd on retransmission (congestion loss detection).
                conn.cwnd = (conn.cwnd / 2).max(MAX_SEGMENT_SIZE);
                conn.ssthresh = conn.cwnd;
                serial_println!(
                    "[TCP] Retransmitting seq={} attempt={}",
                    entry.seq,
                    entry.attempts
                );
            }
        }

        // Remove segments that exceeded max retransmit attempts.
        conn.retransmit_queue
            .retain(|entry| entry.attempts < MAX_RETRANSMIT_ATTEMPTS);

        // ── Keepalive check ──
        let idle_ticks = now.saturating_sub(conn.last_recv_tick);
        let send_keepalive =
            idle_ticks >= KEEPALIVE_INTERVAL_TICKS && conn.retransmit_queue.is_empty();

        if send_keepalive {
            serial_println!(
                "[TCP] Sending keepalive: {} -> {:?}:{}",
                conn.local_port,
                super::FormatIp(conn.remote_addr),
                conn.remote_port
            );
            conn.last_send_tick = now;
        }

        // Collect keepalive info before dropping the borrow.
        let keepalive_info = if send_keepalive {
            Some((
                conn.local_port,
                conn.remote_port,
                conn.seq_num.wrapping_sub(1),
                conn.ack_num,
                conn.recv_window,
                conn.remote_addr,
            ))
        } else {
            None
        };

        // Send retransmitted segments.
        for (seq, data, remote_addr) in &retransmit_segments {
            if let Some(dst_mac) = super::arp_lookup(*remote_addr) {
                let local_ip = super::local_ip();
                let frame = build_tcp_frame(dst_mac, local_ip, *remote_addr, data);
                let _ = crate::drivers::net::send_frame(&frame);
                serial_println!(
                    "[TCP] Retransmitted segment seq={} ({} bytes)",
                    seq,
                    data.len()
                );
            }
        }

        // Send keepalive probe if needed.
        if let Some((lp, rp, seq, ack, window, addr)) = keepalive_info {
            let keep_segment = build_tcp(lp, rp, seq, ack, TCP_FLAG_ACK, window, &[]);
            if let Some(dst_mac) = super::arp_lookup(addr) {
                let local_ip = super::local_ip();
                let frame = build_tcp_frame(dst_mac, local_ip, addr, &keep_segment);
                let _ = crate::drivers::net::send_frame(&frame);
            }
        }
    }
}

// ─────────────────── Tests ───────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tcp_valid() {
        let mut data = vec![0u8; TCP_HEADER_MIN_SIZE];
        write_u16_be(&mut data, 0, 12345); // src_port
        write_u16_be(&mut data, 2, 80); // dst_port
        write_u32_be(&mut data, 4, 1000); // seq
        write_u32_be(&mut data, 8, 2000); // ack
        data[12] = TCP_DATA_OFFSET_5 << 4; // data offset
        data[13] = TCP_FLAG_ACK; // flags
        write_u16_be(&mut data, 14, 65535); // window

        let (header, payload) = parse_tcp(&data).unwrap();
        assert_eq!(header.src_port, 12345);
        assert_eq!(header.dst_port, 80);
        assert_eq!(header.seq, 1000);
        assert_eq!(header.ack, 2000);
        assert_eq!(header.flags, TCP_FLAG_ACK);
        assert_eq!(header.window, 65535);
        assert_eq!(header.data_offset, TCP_HEADER_MIN_SIZE);
        assert!(payload.is_empty());
    }

    #[test]
    fn test_parse_tcp_with_payload() {
        let mut data = vec![0u8; TCP_HEADER_MIN_SIZE + 4];
        write_u16_be(&mut data, 0, 80);
        write_u16_be(&mut data, 2, 12345);
        data[12] = TCP_DATA_OFFSET_5 << 4;
        data[13] = TCP_FLAG_PSH | TCP_FLAG_ACK;
        data[TCP_HEADER_MIN_SIZE] = 0xDE;
        data[TCP_HEADER_MIN_SIZE + 1] = 0xAD;
        data[TCP_HEADER_MIN_SIZE + 2] = 0xBE;
        data[TCP_HEADER_MIN_SIZE + 3] = 0xEF;

        let (header, payload) = parse_tcp(&data).unwrap();
        assert_eq!(header.src_port, 80);
        assert_eq!(payload.len(), 4);
        assert_eq!(payload, [0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn test_parse_tcp_too_short() {
        assert!(parse_tcp(&[0u8; 10]).is_none());
    }

    #[test]
    fn test_parse_tcp_invalid_offset() {
        let mut data = vec![0u8; TCP_HEADER_MIN_SIZE];
        data[12] = 3 << 4; // data offset = 3 (too small, min is 5)
        assert!(parse_tcp(&data).is_none());
    }

    #[test]
    fn test_build_tcp_basic() {
        let segment = build_tcp(12345, 80, 1000, 2000, TCP_FLAG_SYN, 65535, &[]);
        assert_eq!(segment.len(), TCP_HEADER_MIN_SIZE);
        assert_eq!(read_u16_be(&segment, 0), 12345);
        assert_eq!(read_u16_be(&segment, 2), 80);
        assert_eq!(read_u32_be(&segment, 4), 1000);
        assert_eq!(read_u32_be(&segment, 8), 2000);
        assert_eq!(segment[12], TCP_DATA_OFFSET_5 << 4);
        assert_eq!(segment[13], TCP_FLAG_SYN);
        assert_eq!(read_u16_be(&segment, 14), 65535);
    }

    #[test]
    fn test_build_tcp_with_payload() {
        let payload = [0x01, 0x02, 0x03];
        let segment = build_tcp(80, 12345, 2000, 1003, TCP_FLAG_ACK, 65535, &payload);
        assert_eq!(segment.len(), TCP_HEADER_MIN_SIZE + 3);
        assert_eq!(&segment[TCP_HEADER_MIN_SIZE..], &payload);
    }

    #[test]
    fn test_tcp_state_transitions() {
        // Verify all states are distinct.
        let states = [
            TcpState::Closed,
            TcpState::SynSent,
            TcpState::SynReceived,
            TcpState::Established,
            TcpState::FinWait1,
            TcpState::FinWait2,
            TcpState::CloseWait,
            TcpState::LastAck,
            TcpState::TimeWait,
            TcpState::Closing,
        ];
        for i in 0..states.len() {
            for j in (i + 1)..states.len() {
                assert_ne!(states[i], states[j]);
            }
        }
    }

    #[test]
    fn test_tcp_flag_constants() {
        assert_eq!(TCP_FLAG_FIN, 0x01);
        assert_eq!(TCP_FLAG_SYN, 0x02);
        assert_eq!(TCP_FLAG_RST, 0x04);
        assert_eq!(TCP_FLAG_PSH, 0x08);
        assert_eq!(TCP_FLAG_ACK, 0x10);
    }

    #[test]
    fn test_ip_proto_tcp() {
        assert_eq!(IP_PROTO_TCP, 6);
    }

    #[test]
    fn test_build_tcp_roundtrip() {
        let segment = build_tcp(
            12345,
            80,
            1000,
            2000,
            TCP_FLAG_SYN | TCP_FLAG_ACK,
            65535,
            &[],
        );
        let (header, payload) = parse_tcp(&segment).unwrap();
        assert_eq!(header.src_port, 12345);
        assert_eq!(header.dst_port, 80);
        assert_eq!(header.seq, 1000);
        assert_eq!(header.ack, 2000);
        assert_eq!(header.flags, TCP_FLAG_SYN | TCP_FLAG_ACK);
        assert_eq!(header.window, 65535);
        assert!(payload.is_empty());
    }

    #[test]
    fn test_is_seq_before() {
        assert!(is_seq_before(1, 2));
        assert!(!is_seq_before(2, 1));
        assert!(!is_seq_before(1, 1));
        // Test wrap-around.
        assert!(is_seq_before(u32::MAX, 0));
        assert!(!is_seq_before(0, u32::MAX));
    }

    #[test]
    fn test_parse_tcp_syn() {
        let mut data = vec![0u8; TCP_HEADER_MIN_SIZE];
        write_u16_be(&mut data, 0, 49152);
        write_u16_be(&mut data, 2, 80);
        write_u32_be(&mut data, 4, INITIAL_SEQ);
        data[12] = TCP_DATA_OFFSET_5 << 4;
        data[13] = TCP_FLAG_SYN;
        write_u16_be(&mut data, 14, DEFAULT_WINDOW_SIZE);

        let (header, payload) = parse_tcp(&data).unwrap();
        assert_eq!(header.flags & TCP_FLAG_SYN, TCP_FLAG_SYN);
        assert_eq!(header.flags & TCP_FLAG_ACK, 0);
        assert_eq!(header.seq, INITIAL_SEQ);
        assert!(payload.is_empty());
    }

    #[test]
    fn test_parse_tcp_fin_ack() {
        let mut data = vec![0u8; TCP_HEADER_MIN_SIZE];
        write_u16_be(&mut data, 0, 80);
        write_u16_be(&mut data, 2, 49152);
        write_u32_be(&mut data, 4, 5000);
        write_u32_be(&mut data, 8, 1001);
        data[12] = TCP_DATA_OFFSET_5 << 4;
        data[13] = TCP_FLAG_FIN | TCP_FLAG_ACK;
        write_u16_be(&mut data, 14, 1024);

        let (header, _) = parse_tcp(&data).unwrap();
        assert_eq!(header.flags & TCP_FLAG_FIN, TCP_FLAG_FIN);
        assert_eq!(header.flags & TCP_FLAG_ACK, TCP_FLAG_ACK);
    }

    // ─────────────────── Listen / Accept tests ───────────────────

    #[test]
    fn test_register_listen_port() {
        // Clean up from other tests.
        unregister_listen_port(8080);

        register_listen_port(8080);
        assert!(has_pending_accept(8080) || PENDING_ACCEPT.lock().contains_key(&8080));

        // Registering the same port again should not panic.
        register_listen_port(8080);

        unregister_listen_port(8080);
        assert!(!PENDING_ACCEPT.lock().contains_key(&8080));
    }

    #[test]
    fn test_unregister_nonexistent_port() {
        // Should not panic.
        unregister_listen_port(9999);
    }

    #[test]
    fn test_accept_next_empty_queue() {
        register_listen_port(7777);
        assert!(accept_next(7777).is_none());
        unregister_listen_port(7777);
    }

    #[test]
    fn test_accept_next_after_enqueue() {
        register_listen_port(7778);

        // Simulate enqueuing a connection (as handle_passive_syn would).
        {
            let mut pending = PENDING_ACCEPT.lock();
            if let Some(queue) = pending.get_mut(&7778) {
                queue.push((0x0100A8C0, 54321));
            }
        }

        assert!(has_pending_accept(7778));

        let result = accept_next(7778);
        assert!(result.is_some());
        let (addr, port) = result.unwrap();
        assert_eq!(addr, 0x0100A8C0);
        assert_eq!(port, 54321);

        // Queue should now be empty.
        assert!(!has_pending_accept(7778));

        unregister_listen_port(7778);
    }

    #[test]
    fn test_accept_fifo_order() {
        register_listen_port(7779);

        {
            let mut pending = PENDING_ACCEPT.lock();
            if let Some(queue) = pending.get_mut(&7779) {
                queue.push((0x0A000001, 11111));
                queue.push((0x0A000002, 22222));
                queue.push((0x0A000003, 33333));
            }
        }

        // Should dequeue in FIFO order.
        let (a1, p1) = accept_next(7779).unwrap();
        assert_eq!(a1, 0x0A000001);
        assert_eq!(p1, 11111);

        let (a2, p2) = accept_next(7779).unwrap();
        assert_eq!(a2, 0x0A000002);
        assert_eq!(p2, 22222);

        let (a3, p3) = accept_next(7779).unwrap();
        assert_eq!(a3, 0x0A000003);
        assert_eq!(p3, 33333);

        assert!(!has_pending_accept(7779));
        unregister_listen_port(7779);
    }

    #[test]
    fn test_has_pending_accept_unregistered_port() {
        // A port that was never registered.
        assert!(!has_pending_accept(12345));
    }

    #[test]
    fn test_listen_port_isolation() {
        // Ports should be independent.
        register_listen_port(8001);
        register_listen_port(8002);

        {
            let mut pending = PENDING_ACCEPT.lock();
            if let Some(queue) = pending.get_mut(&8001) {
                queue.push((0x0A000001, 100));
            }
        }

        assert!(has_pending_accept(8001));
        assert!(!has_pending_accept(8002));

        unregister_listen_port(8001);
        unregister_listen_port(8002);
    }

    #[test]
    fn test_is_seq_before_wrap_around() {
        assert!(is_seq_before(u32::MAX - 1, 0));
        assert!(is_seq_before(u32::MAX, 1));
        assert!(!is_seq_before(0, u32::MAX));
    }

    #[test]
    fn test_is_seq_before_half_space() {
        let mid = 1u32 << 31;
        assert!(is_seq_before(0, mid - 1));
        assert!(!is_seq_before(0, mid + 1));
    }

    #[test]
    fn test_seq_wrapping_add() {
        let seq: u32 = u32::MAX - 5;
        assert_eq!(seq.wrapping_add(10), 4);
    }

    #[test]
    fn test_internet_checksum_empty() {
        assert_eq!(internet_checksum(&[]), 0xFFFF);
    }

    #[test]
    fn test_internet_checksum_odd_length() {
        assert_eq!(internet_checksum(&[0x01]), 0xFEFF);
    }

    #[test]
    fn test_tcp_checksum_deterministic() {
        let segment = build_tcp(12345, 80, 1000, 2000, TCP_FLAG_SYN, 65535, &[]);
        let c1 = tcp_checksum(0xC0A80001, 0xC0A80002, &segment);
        let c2 = tcp_checksum(0xC0A80001, 0xC0A80002, &segment);
        assert_eq!(c1, c2);
    }

    #[test]
    fn test_tcp_checksum_differs_with_different_ip() {
        let segment = build_tcp(12345, 80, 1000, 2000, TCP_FLAG_SYN, 65535, &[]);
        let c1 = tcp_checksum(0xC0A80001, 0xC0A80002, &segment);
        let c2 = tcp_checksum(0xC0A80001, 0xC0A80003, &segment);
        assert_ne!(c1, c2);
    }

    #[test]
    fn test_build_tcp_zero_window() {
        let segment = build_tcp(80, 12345, 0, 0, TCP_FLAG_ACK, 0, &[]);
        let (header, _) = parse_tcp(&segment).unwrap();
        assert_eq!(header.window, 0);
    }

    #[test]
    fn test_build_tcp_max_seq() {
        let segment = build_tcp(80, 12345, u32::MAX, u32::MAX, TCP_FLAG_ACK, 65535, &[]);
        let (header, _) = parse_tcp(&segment).unwrap();
        assert_eq!(header.seq, u32::MAX);
    }

    #[test]
    fn test_parse_tcp_data_offset_6() {
        let mut data = vec![0u8; 24];
        data[12] = 6 << 4;
        data[13] = TCP_FLAG_ACK;
        let (header, _) = parse_tcp(&data).unwrap();
        assert_eq!(header.data_offset, 24);
    }

    #[test]
    fn test_new_connection_duplicate_key() {
        let lp = 51000u16;
        let ra = 0x0100A8C0u32;
        let rp = 80u16;
        let _ = remove_connection(lp, ra, rp);
        assert!(new_connection(lp, ra, rp).is_ok());
        assert!(new_connection(lp, ra, rp).is_err());
        let _ = remove_connection(lp, ra, rp);
    }

    #[test]
    fn test_remove_connection_twice() {
        let lp = 51002u16;
        let ra = 0x0100A8C0u32;
        let rp = 80u16;
        let _ = remove_connection(lp, ra, rp);
        new_connection(lp, ra, rp).unwrap();
        assert!(remove_connection(lp, ra, rp));
        assert!(!remove_connection(lp, ra, rp));
    }

    #[test]
    fn test_read_write_u16_be_roundtrip() {
        let mut buf = [0u8; 2];
        write_u16_be(&mut buf, 0, 0x1234);
        assert_eq!(read_u16_be(&buf, 0), 0x1234);
    }

    #[test]
    fn test_read_write_u32_be_roundtrip() {
        let mut buf = [0u8; 4];
        write_u32_be(&mut buf, 0, 0xDEADBEEF);
        assert_eq!(read_u32_be(&buf, 0), 0xDEADBEEF);
    }

    #[test]
    fn test_tcp_state_display() {
        // Verify all states can be debug-formatted.
        let states = [
            TcpState::SynSent,
            TcpState::Established,
            TcpState::FinWait1,
            TcpState::Closed,
        ];
        for s in &states {
            let _ = format!("{s:?}");
        }
    }

    #[test]
    fn test_tcp_header_defaults() {
        let hdr = TcpHeader {
            src_port: 1234,
            dst_port: 80,
            seq: 0,
            ack: 0,
            data_offset: 5,
            flags: 0,
            window: 65535,
            checksum: 0,
            urgent: 0,
        };
        assert_eq!(hdr.src_port, 1234);
        assert_eq!(hdr.dst_port, 80);
        assert_eq!(hdr.data_offset, 5);
    }

    #[test]
    fn test_parse_tcp_short_data() {
        let data = [0u8; 10]; // Less than 20 bytes minimum.
        assert!(parse_tcp(&data).is_none());
    }

    #[test]
    fn test_allocate_local_port_returns_some() {
        let port = allocate_local_port();
        assert!(port.is_some());
        assert!(port.unwrap() >= 49152);
    }
}
