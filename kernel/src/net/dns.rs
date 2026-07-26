//! DNS resolver (RFC 1035).
//!
//! Implements a simple DNS stub resolver that sends A record queries to a
//! DNS server via UDP and parses the response to extract IPv4 addresses.
//!
//! ## Protocol Flow
//!
//! ```text
//!   Client                          DNS Server
//!     │                                │
//!     │  DNS Query (UDP port 53)       │
//!     │  ───────────────────────────→  │
//!     │                                │
//!     │  DNS Response                  │
//!     │  ←───────────────────────────  │
//!     │                                │
//!     │  (IPv4 address extracted)      │
//! ```
//!
//! ## DNS Message Format (RFC 1035 §4.1.1)
//!
//! ```text
//!  0                   1                   2                   3
//!  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |          ID (16 bits)         |        FLAGS (16 bits)        |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |        QDCOUNT (16 bits)      |       ANCOUNT (16 bits)       |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |        NSCOUNT (16 bits)      |       ARCOUNT (16 bits)       |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                      Questions (variable)                     |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                      Answers (variable)                       |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! ```
//!
//! ## Limitations
//!
//! - Only resolves A records (IPv4 addresses).
//! - No support for CNAME following, AAAA, or other record types.
//! - Uses a fixed transaction ID (not randomized).
//! - Does not implement DNS compression (pointer labels).

use alloc::vec;
use alloc::vec::Vec;

use crate::serial_println;

// ---------------------------------------------------------------------------
// DNS Constants (RFC 1035)
// ---------------------------------------------------------------------------

/// DNS query class: Internet (IN).
const DNS_CLASS_IN: u16 = 1;

/// DNS query type: A record (IPv4 address).
const DNS_TYPE_A: u16 = 1;

/// DNS header size in bytes (12 bytes).
const DNS_HEADER_SIZE: usize = 12;

/// DNS standard query flags (QR=0, Opcode=0, RD=1).
///
/// Bit layout: 0 0000 0 0 1 0 000 0000
/// - QR (bit 15): 0 = query
/// - Opcode (bits 14-11): 0 = standard query
/// - AA (bit 10): 0 = not authoritative
/// - TC (bit 9): 0 = not truncated
/// - RD (bit 8): 1 = recursion desired
/// - RA (bit 7): 0 (ignored in query)
/// - Z (bits 6-4): 0
/// - RCODE (bits 3-0): 0
const DNS_QUERY_FLAGS: u16 = 0x0100;

/// DNS response flags mask: QR bit (bit 15).
const DNS_FLAG_QR: u16 = 0x8000;

/// DNS response flags mask: response code (bits 3-0).
const DNS_FLAG_RCODE_MASK: u16 = 0x000F;

/// DNS UDP port.
const DNS_PORT: u16 = 53;

/// DNS timeout in timer ticks (~18.2 Hz, so ~100 ticks ≈ 5.5 seconds).
/// We use 36 ticks ≈ 2 seconds per attempt.
const DNS_TIMEOUT_TICKS: u64 = 36;

/// Maximum DNS retries.
const DNS_MAX_RETRIES: u32 = 3;

/// Maximum DNS response size (UDP payload limit).
const DNS_MAX_RESPONSE_SIZE: usize = 512;

/// Fallback DNS server: Google Public DNS (8.8.8.8) in network byte order.
const FALLBACK_DNS_SERVER: [u8; 4] = [8, 8, 8, 8];

/// DNS transaction ID (fixed for simplicity).
const DNS_TRANSACTION_ID: u16 = 0xABCD;

// ---------------------------------------------------------------------------
// DNS Structures
// ---------------------------------------------------------------------------

/// DNS header (RFC 1035 §4.1.1).
///
/// All fields are in host byte order.
#[derive(Debug, Clone, Copy)]
struct DnsHeader {
    /// Transaction ID.
    id: u16,
    /// Flags (QR, Opcode, AA, TC, RD, RA, Z, RCODE).
    flags: u16,
    /// Number of entries in the question section.
    qdcount: u16,
    /// Number of entries in the answer section.
    ancount: u16,
    /// Number of entries in the authority section.
    nscount: u16,
    /// Number of entries in the additional section.
    arcount: u16,
}

/// DNS question section entry (RFC 1035 §4.1.2).
///
/// Represents a single question in the DNS query.
#[derive(Debug, Clone)]
struct DnsQuestion {
    /// Domain name (as DNS labels, e.g., [3, 'w', 'w', 'w', 7, 'e', 'x', ...]).
    name: Vec<u8>,
    /// Query type (e.g., 1 for A record).
    qtype: u16,
    /// Query class (e.g., 1 for IN).
    qclass: u16,
}

/// DNS answer section entry (RFC 1035 §4.1.3).
///
/// Represents a single answer from the DNS response.
#[derive(Debug, Clone)]
struct DnsAnswer {
    /// Domain name (may be a pointer to the question).
    name: Vec<u8>,
    /// Resource record type (e.g., 1 for A record).
    rtype: u16,
    /// Resource record class (e.g., 1 for IN).
    rclass: u16,
    /// Time to live (seconds).
    ttl: u32,
    /// Resource data (e.g., 4 bytes for A record).
    rdata: Vec<u8>,
}

/// DNS error types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsError {
    /// DNS server returned an error code.
    ServerError(u8),
    /// Response ID did not match query ID.
    IdMismatch,
    /// Response is not a response (QR bit not set).
    NotResponse,
    /// No A record found in the response.
    NoARecord,
    /// Response was truncated.
    Truncated,
    /// Network error (failed to send/receive).
    NetworkError,
    /// Timeout waiting for response.
    Timeout,
    /// Invalid response format.
    InvalidResponse,
    /// No DNS server configured.
    NoServer,
    /// Hostname too long (max 253 characters).
    HostnameTooLong,
    /// Empty hostname.
    EmptyHostname,
}

// ---------------------------------------------------------------------------
// DNS Label Encoding
// ---------------------------------------------------------------------------

/// Encode a domain name as DNS labels (RFC 1035 §4.1.4).
///
/// Converts a dotted domain name (e.g., "www.example.com") into the DNS
/// wire format: length-prefixed segments followed by a zero-length terminator.
///
/// # Arguments
///
/// * `hostname` - The domain name in dotted notation.
///
/// # Returns
///
/// A `Vec<u8>` containing the encoded domain name.
///
/// # Errors
///
/// Returns `DnsError::EmptyHostname` if the hostname is empty.
/// Returns `DnsError::HostnameTooLong` if the hostname exceeds 253 characters.
fn encode_domain_name(hostname: &str) -> Result<Vec<u8>, DnsError> {
    if hostname.is_empty() {
        return Err(DnsError::EmptyHostname);
    }

    // RFC 1035 §2.3.4: max domain name length is 253 characters.
    if hostname.len() > 253 {
        return Err(DnsError::HostnameTooLong);
    }

    let mut encoded = Vec::new();

    // Handle trailing dot (optional).
    let name = hostname.strip_suffix('.').unwrap_or(hostname);

    // Split by dots and encode each label.
    for label in name.split('.') {
        let label_len = label.len();

        // RFC 1035 §2.3.4: max label length is 63 characters.
        if label_len > 63 {
            return Err(DnsError::HostnameTooLong);
        }

        if label_len == 0 {
            return Err(DnsError::EmptyHostname);
        }

        // Length byte followed by label bytes.
        encoded.push(label_len as u8);
        encoded.extend_from_slice(label.as_bytes());
    }

    // Zero-length terminator.
    encoded.push(0);

    Ok(encoded)
}

// ---------------------------------------------------------------------------
// DNS Query Construction
// ---------------------------------------------------------------------------

/// Build a DNS query packet for an A record.
///
/// Constructs a standard DNS query with recursion desired for the given
/// hostname.
///
/// # Arguments
///
/// * `hostname` - The domain name to resolve.
///
/// # Returns
///
/// A `Vec<u8>` containing the DNS query packet.
fn build_dns_query(hostname: &str) -> Result<Vec<u8>, DnsError> {
    let encoded_name = encode_domain_name(hostname)?;

    // Total packet size: header + question (name + type + class).
    let question_len = encoded_name.len() + 4; // +4 for qtype and qclass
    let packet_len = DNS_HEADER_SIZE + question_len;
    let mut packet = vec![0u8; packet_len];

    // Write header.
    write_u16_be(&mut packet, 0, DNS_TRANSACTION_ID);
    write_u16_be(&mut packet, 2, DNS_QUERY_FLAGS);
    write_u16_be(&mut packet, 4, 1); // QDCOUNT = 1
    write_u16_be(&mut packet, 6, 0); // ANCOUNT = 0
    write_u16_be(&mut packet, 8, 0); // NSCOUNT = 0
    write_u16_be(&mut packet, 10, 0); // ARCOUNT = 0

    // Write question section.
    let question_offset = DNS_HEADER_SIZE;
    packet[question_offset..question_offset + encoded_name.len()].copy_from_slice(&encoded_name);
    write_u16_be(
        &mut packet,
        question_offset + encoded_name.len(),
        DNS_TYPE_A,
    );
    write_u16_be(
        &mut packet,
        question_offset + encoded_name.len() + 2,
        DNS_CLASS_IN,
    );

    Ok(packet)
}

// ---------------------------------------------------------------------------
// DNS Response Parsing
// ---------------------------------------------------------------------------

/// Parse a DNS header from `data`.
///
/// Returns `None` if the data is too short.
fn parse_dns_header(data: &[u8]) -> Option<DnsHeader> {
    if data.len() < DNS_HEADER_SIZE {
        return None;
    }

    Some(DnsHeader {
        id: read_u16_be(data, 0),
        flags: read_u16_be(data, 2),
        qdcount: read_u16_be(data, 4),
        ancount: read_u16_be(data, 6),
        nscount: read_u16_be(data, 8),
        arcount: read_u16_be(data, 10),
    })
}

/// Skip over a domain name in the DNS packet.
///
/// Handles both standard labels and compression pointers (RFC 1035 §4.1.4).
/// Returns the offset after the name.
fn skip_domain_name(data: &[u8], offset: usize) -> Option<usize> {
    let mut pos = offset;

    loop {
        if pos >= data.len() {
            return None;
        }

        let len = data[pos];

        // Compression pointer: two bytes with top two bits set.
        if (len & 0xC0) == 0xC0 {
            // Pointer is 2 bytes total.
            return Some(pos + 2);
        }

        // Zero-length terminator.
        if len == 0 {
            return Some(pos + 1);
        }

        // Standard label: skip length byte + label bytes.
        pos += 1 + (len as usize);

        if pos > data.len() {
            return None;
        }
    }
}

/// Parse the question section from a DNS response.
///
/// Skips `qdcount` questions starting at `offset`.
/// Returns the offset after all questions.
fn parse_questions(data: &[u8], offset: usize, qdcount: u16) -> Option<usize> {
    let mut pos = offset;

    for _ in 0..qdcount {
        // Skip the domain name.
        pos = skip_domain_name(data, pos)?;

        // Skip QTYPE (2 bytes) and QCLASS (2 bytes).
        if pos + 4 > data.len() {
            return None;
        }
        pos += 4;
    }

    Some(pos)
}

/// Parse a single answer from the DNS response.
///
/// Returns the parsed `DnsAnswer` and the offset after the answer.
fn parse_answer(data: &[u8], offset: usize) -> Option<(DnsAnswer, usize)> {
    let mut pos = offset;

    // Parse or skip the domain name.
    let name_start = pos;
    pos = skip_domain_name(data, pos)?;

    // Extract the name bytes for the answer.
    let name = data[name_start..pos].to_vec();

    // Check if we have enough bytes for the fixed fields.
    if pos + 10 > data.len() {
        return None;
    }

    // Parse fixed fields.
    let rtype = read_u16_be(data, pos);
    let rclass = read_u16_be(data, pos + 2);
    let ttl = read_u32_be(data, pos + 4);
    let rdlength = read_u16_be(data, pos + 8) as usize;
    pos += 10;

    // Check if RDATA fits.
    if pos + rdlength > data.len() {
        return None;
    }

    // Extract RDATA.
    let rdata = data[pos..pos + rdlength].to_vec();
    pos += rdlength;

    Some((
        DnsAnswer {
            name,
            rtype,
            rclass,
            ttl,
            rdata,
        },
        pos,
    ))
}

/// Parse the answer section from a DNS response.
///
/// Returns a vector of `DnsAnswer` entries.
fn parse_answers(data: &[u8], offset: usize, ancount: u16) -> Option<Vec<DnsAnswer>> {
    let mut answers = Vec::new();
    let mut pos = offset;

    for _ in 0..ancount {
        let (answer, new_pos) = parse_answer(data, pos)?;
        answers.push(answer);
        pos = new_pos;
    }

    Some(answers)
}

/// Find the first A record (type 1) in the answer section.
///
/// Returns the IPv4 address (4 bytes) if found.
fn find_a_record(answers: &[DnsAnswer]) -> Option<[u8; 4]> {
    for answer in answers {
        // Type A (1), Class IN (1), RDATA length 4.
        if answer.rtype == DNS_TYPE_A && answer.rclass == DNS_CLASS_IN && answer.rdata.len() == 4 {
            let mut ip = [0u8; 4];
            ip.copy_from_slice(&answer.rdata);
            return Some(ip);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// DNS Resolution
// ---------------------------------------------------------------------------

/// Resolve a hostname to an IPv4 address using DNS.
///
/// Sends a DNS A record query to the configured DNS server and parses the
/// response to extract the IPv4 address. Retries up to 3 times with a
/// 2-second timeout per attempt.
///
/// # Arguments
///
/// * `hostname` - The domain name to resolve (e.g., "example.com").
///
/// # Returns
///
/// * `Ok(ip)` - The resolved IPv4 address in network byte order.
/// * `Err(DnsError)` - The error that occurred.
///
/// # Examples
///
/// ```rust,ignore
/// let ip = dns::resolve("example.com")?;
/// // ip is a 4-byte array in network byte order
/// ```
/// Resolve a hostname to an IPv4 address using DNS.
///
/// Sends a DNS A record query to the configured DNS server and parses the
/// response to extract the IPv4 address. Retries up to 3 times with a
/// 2-second timeout per attempt.
///
/// # Arguments
///
/// * `hostname` - The domain name to resolve (e.g., "example.com").
///
/// # Returns
///
/// * `Ok(ip)` - The resolved IPv4 address in network byte order.
/// * `Err(DnsError)` - The error that occurred.
///
/// # Errors
///
/// Returns `DnsError` if resolution fails due to network errors, timeouts,
/// or invalid responses.
pub fn resolve(hostname: &str) -> Result<[u8; 4], DnsError> {
    serial_println!("[DNS] Resolving '{}'", hostname);

    // Build the query packet.
    let query_packet = build_dns_query(hostname)?;

    // Get the DNS server address.
    let dns_server = get_dns_server();
    serial_println!(
        "[DNS] Using server {}.{}.{}.{}",
        dns_server[0],
        dns_server[1],
        dns_server[2],
        dns_server[3]
    );

    // Send query with retries.
    for attempt in 0..DNS_MAX_RETRIES {
        serial_println!(
            "[DNS] Attempt {}/{} for '{}'",
            attempt + 1,
            DNS_MAX_RETRIES,
            hostname
        );

        match send_query_and_wait(&query_packet, dns_server) {
            Ok(ip) => {
                serial_println!(
                    "[DNS] Resolved '{}': {}.{}.{}.{}",
                    hostname,
                    ip[0],
                    ip[1],
                    ip[2],
                    ip[3]
                );
                return Ok(ip);
            }
            Err(e) => {
                serial_println!("[DNS] Attempt {} failed: {:?}", attempt + 1, e);

                // If this was the last attempt, return the error.
                if attempt == DNS_MAX_RETRIES - 1 {
                    return Err(e);
                }
            }
        }
    }

    // Unreachable, but Rust requires a return value.
    Err(DnsError::Timeout)
}

/// Get the DNS server address from DHCP state, or fallback to 8.8.8.8.
fn get_dns_server() -> [u8; 4] {
    let state = super::dhcp::get_network_state();

    // Check if DHCP provided a DNS server (non-zero).
    if state.dns == [0, 0, 0, 0] {
        serial_println!("[DNS] No DHCP DNS server, using fallback 8.8.8.8");
        FALLBACK_DNS_SERVER
    } else {
        state.dns
    }
}

/// Send a DNS query and wait for a response.
///
/// Sends the query packet to the DNS server via UDP and waits for a response
/// with timeout.
///
/// # Arguments
///
/// * `query_packet` - The DNS query packet to send.
/// * `dns_server` - The DNS server's IPv4 address (network byte order).
///
/// # Returns
///
/// * `Ok(ip)` - The resolved IPv4 address.
/// * `Err(DnsError)` - The error that occurred.
fn send_query_and_wait(query_packet: &[u8], dns_server: [u8; 4]) -> Result<[u8; 4], DnsError> {
    // Build the UDP packet.
    let local_ip = super::local_ip().to_be_bytes();
    let udp_packet = super::udp::build_udp(
        local_ip,
        dns_server,
        DNS_PORT, // Source port (use DNS port for simplicity)
        DNS_PORT,
        query_packet,
    );

    // Resolve the DNS server's MAC address via ARP.
    let Some(dst_mac) = super::arp_lookup(u32::from_be_bytes(dns_server)) else {
        serial_println!("[DNS] No ARP entry for DNS server, sending ARP request");
        super::send_arp_request(u32::from_be_bytes(dns_server));

        // Wait for ARP resolution.
        wait_for_arp(u32::from_be_bytes(dns_server))?;

        // Try again after ARP resolution.
        let dst_mac =
            super::arp_lookup(u32::from_be_bytes(dns_server)).ok_or(DnsError::NetworkError)?;
        return send_udp_and_parse_response(&udp_packet, dst_mac, dns_server);
    };

    send_udp_and_parse_response(&udp_packet, dst_mac, dns_server)
}

/// Wait for an ARP entry to be populated.
///
/// Waits up to `DNS_TIMEOUT_TICKS` for the ARP table to be populated.
fn wait_for_arp(target_ip: u32) -> Result<(), DnsError> {
    let start = crate::arch::x86_64::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);

    loop {
        if super::arp_lookup(target_ip).is_some() {
            return Ok(());
        }

        let current =
            crate::arch::x86_64::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);
        if current - start >= DNS_TIMEOUT_TICKS {
            return Err(DnsError::Timeout);
        }

        // Process pending network frames.
        if let Some(frame) = crate::drivers::net::receive_frame() {
            // Handle the frame (this will update ARP table if it's an ARP reply).
            super::handle_frame(&frame);
        }

        x86_64::instructions::hlt();
    }
}

/// Send UDP packet and parse the DNS response.
///
/// # Arguments
///
/// * `udp_packet` - The complete UDP/IP packet.
/// * `dst_mac` - Destination MAC address.
/// * `dns_server` - DNS server IP (for logging).
///
/// # Returns
///
/// * `Ok(ip)` - The resolved IPv4 address.
/// * `Err(DnsError)` - The error that occurred.
fn send_udp_and_parse_response(
    udp_packet: &[u8],
    dst_mac: [u8; 6],
    dns_server: [u8; 4],
) -> Result<[u8; 4], DnsError> {
    // Build Ethernet frame.
    let src_mac = crate::drivers::net::mac_address();
    let frame = super::build_ethernet(dst_mac, src_mac, super::ETHERTYPE_IPV4, udp_packet);

    // Send the frame.
    if let Err(e) = crate::drivers::net::send_frame(&frame) {
        serial_println!("[DNS] Failed to send query: {:?}", e);
        return Err(DnsError::NetworkError);
    }

    serial_println!("[DNS] Query sent, waiting for response...");

    // Wait for response with timeout.
    let start = crate::arch::x86_64::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);

    loop {
        // Check for timeout.
        let current =
            crate::arch::x86_64::interrupts::TICKS.load(core::sync::atomic::Ordering::Relaxed);
        if current - start >= DNS_TIMEOUT_TICKS {
            serial_println!("[DNS] Timeout waiting for response");
            return Err(DnsError::Timeout);
        }

        // Check for incoming frames.
        if let Some(frame) = crate::drivers::net::receive_frame() {
            // Try to parse as a DNS response.
            if let Some(ip) = try_parse_dns_response(&frame, dns_server) {
                return Ok(ip);
            }

            // If not a DNS response, handle it normally (ARP, etc.).
            super::handle_frame(&frame);
        }

        x86_64::instructions::hlt();
    }
}

/// Try to parse an Ethernet frame as a DNS response.
///
/// Returns the resolved IPv4 address if the frame contains a valid DNS
/// response matching our transaction ID.
fn try_parse_dns_response(frame: &[u8], dns_server: [u8; 4]) -> Option<[u8; 4]> {
    // Minimum Ethernet frame size.
    if frame.len() < 14 {
        return None;
    }

    // Check EtherType (bytes 12..14).
    let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
    if ethertype != super::ETHERTYPE_IPV4 {
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

    // Verify UDP protocol (17).
    let protocol = frame[ip_start + 9];
    if protocol != 17 {
        return None;
    }

    // Verify source IP is the DNS server.
    let src_ip = [
        frame[ip_start + 12],
        frame[ip_start + 13],
        frame[ip_start + 14],
        frame[ip_start + 15],
    ];
    if src_ip != dns_server {
        return None;
    }

    // UDP header starts after IP header.
    let udp_start = ip_start + ip_header_len;
    if frame.len() < udp_start + 8 {
        return None;
    }

    // Verify source port is DNS (53).
    let src_port = u16::from_be_bytes([frame[udp_start], frame[udp_start + 1]]);
    if src_port != DNS_PORT {
        return None;
    }

    // UDP length.
    let udp_length = u16::from_be_bytes([frame[udp_start + 4], frame[udp_start + 5]]) as usize;
    if udp_length < 8 {
        return None;
    }

    // DNS payload starts after UDP header.
    let dns_start = udp_start + 8;
    let dns_len = udp_length - 8;

    if frame.len() < dns_start + dns_len {
        return None;
    }

    // Parse the DNS response.
    let dns_data = &frame[dns_start..dns_start + dns_len];
    parse_dns_response(dns_data)
}

/// Parse a DNS response packet.
///
/// Validates the response and extracts the IPv4 address from the first
/// A record in the answer section.
fn parse_dns_response(data: &[u8]) -> Option<[u8; 4]> {
    // Parse header.
    let header = parse_dns_header(data)?;

    // Validate transaction ID.
    if header.id != DNS_TRANSACTION_ID {
        serial_println!(
            "[DNS] ID mismatch: expected {:#x}, got {:#x}",
            DNS_TRANSACTION_ID,
            header.id
        );
        return None;
    }

    // Validate QR bit (must be 1 for response).
    if header.flags & DNS_FLAG_QR == 0 {
        serial_println!("[DNS] Not a response (QR=0)");
        return None;
    }

    // Check response code (bits 3-0).
    let rcode = header.flags & DNS_FLAG_RCODE_MASK;
    if rcode != 0 {
        serial_println!("[DNS] Server error: RCODE={}", rcode);
        return None;
    }

    // Check if response is truncated.
    if header.flags & 0x0200 != 0 {
        serial_println!("[DNS] Response truncated");
        return None;
    }

    // Must have at least one answer.
    if header.ancount == 0 {
        serial_println!("[DNS] No answers in response");
        return None;
    }

    // Parse questions section to find the start of answers.
    let answers_offset = parse_questions(data, DNS_HEADER_SIZE, header.qdcount)?;

    // Parse answers section.
    let answers = parse_answers(data, answers_offset, header.ancount)?;

    // Find the first A record.
    find_a_record(&answers)
}

// ---------------------------------------------------------------------------
// Byte Helpers
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ─── DNS Constants ───

    #[test]
    fn test_dns_constants() {
        assert_eq!(DNS_CLASS_IN, 1);
        assert_eq!(DNS_TYPE_A, 1);
        assert_eq!(DNS_HEADER_SIZE, 12);
        assert_eq!(DNS_QUERY_FLAGS, 0x0100);
        assert_eq!(DNS_FLAG_QR, 0x8000);
        assert_eq!(DNS_FLAG_RCODE_MASK, 0x000F);
        assert_eq!(DNS_PORT, 53);
        assert_eq!(DNS_TRANSACTION_ID, 0xABCD);
    }

    // ─── Domain Name Encoding ───

    #[test]
    fn test_encode_domain_name_simple() {
        let encoded = encode_domain_name("example.com").unwrap();
        // Expected: 7 "example" 3 "com" 0
        assert_eq!(
            encoded,
            vec![7, b'e', b'x', b'a', b'm', b'p', b'l', b'e', 3, b'c', b'o', b'm', 0]
        );
    }

    #[test]
    fn test_encode_domain_name_with_subdomain() {
        let encoded = encode_domain_name("www.example.com").unwrap();
        // Expected: 3 "www" 7 "example" 3 "com" 0
        assert_eq!(
            encoded,
            vec![
                3, b'w', b'w', b'w', 7, b'e', b'x', b'a', b'm', b'p', b'l', b'e', 3, b'c', b'o',
                b'm', 0
            ]
        );
    }

    #[test]
    fn test_encode_domain_name_with_trailing_dot() {
        let encoded = encode_domain_name("example.com.").unwrap();
        // Should be the same as without trailing dot.
        assert_eq!(
            encoded,
            vec![7, b'e', b'x', b'a', b'm', b'p', b'l', b'e', 3, b'c', b'o', b'm', 0]
        );
    }

    #[test]
    fn test_encode_domain_name_empty() {
        assert_eq!(encode_domain_name(""), Err(DnsError::EmptyHostname));
    }

    #[test]
    fn test_encode_domain_name_too_long() {
        // Create a hostname that's too long (> 253 characters).
        let long_hostname = "a".repeat(254);
        assert_eq!(
            encode_domain_name(&long_hostname),
            Err(DnsError::HostnameTooLong)
        );
    }

    #[test]
    fn test_encode_domain_name_label_too_long() {
        // Create a label that's too long (> 63 characters).
        let long_label = "a".repeat(64);
        let hostname = format!("{}.com", long_label);
        assert_eq!(
            encode_domain_name(&hostname),
            Err(DnsError::HostnameTooLong)
        );
    }

    #[test]
    fn test_encode_domain_name_single_label() {
        let encoded = encode_domain_name("localhost").unwrap();
        assert_eq!(
            encoded,
            vec![9, b'l', b'o', b'c', b'a', b'l', b'h', b'o', b's', b't', 0]
        );
    }

    // ─── DNS Query Building ───

    #[test]
    fn test_build_dns_query() {
        let query = build_dns_query("example.com").unwrap();

        // Verify header.
        assert_eq!(read_u16_be(&query, 0), DNS_TRANSACTION_ID);
        assert_eq!(read_u16_be(&query, 2), DNS_QUERY_FLAGS);
        assert_eq!(read_u16_be(&query, 4), 1); // QDCOUNT
        assert_eq!(read_u16_be(&query, 6), 0); // ANCOUNT
        assert_eq!(read_u16_be(&query, 8), 0); // NSCOUNT
        assert_eq!(read_u16_be(&query, 10), 0); // ARCOUNT

        // Verify question section.
        let qname_offset = DNS_HEADER_SIZE;
        // Check encoded name.
        assert_eq!(query[qname_offset], 7); // "example" length
        assert_eq!(&query[qname_offset + 1..qname_offset + 8], b"example");
        assert_eq!(query[qname_offset + 8], 3); // "com" length
        assert_eq!(&query[qname_offset + 9..qname_offset + 12], b"com");
        assert_eq!(query[qname_offset + 12], 0); // terminator

        // Check QTYPE and QCLASS.
        assert_eq!(read_u16_be(&query, qname_offset + 13), DNS_TYPE_A);
        assert_eq!(read_u16_be(&query, qname_offset + 15), DNS_CLASS_IN);
    }

    // ─── DNS Header Parsing ───

    #[test]
    fn test_parse_dns_header_valid() {
        let mut data = vec![0u8; DNS_HEADER_SIZE];
        write_u16_be(&mut data, 0, 0x1234); // ID
        write_u16_be(&mut data, 2, 0x8180); // Flags (QR=1, RD=1, RA=1)
        write_u16_be(&mut data, 4, 1); // QDCOUNT
        write_u16_be(&mut data, 6, 1); // ANCOUNT
        write_u16_be(&mut data, 8, 0); // NSCOUNT
        write_u16_be(&mut data, 10, 0); // ARCOUNT

        let header = parse_dns_header(&data).unwrap();
        assert_eq!(header.id, 0x1234);
        assert_eq!(header.flags, 0x8180);
        assert_eq!(header.qdcount, 1);
        assert_eq!(header.ancount, 1);
        assert_eq!(header.nscount, 0);
        assert_eq!(header.arcount, 0);
    }

    #[test]
    fn test_parse_dns_header_too_short() {
        assert!(parse_dns_header(&[0u8; 11]).is_none());
    }

    // ─── Domain Name Skipping ───

    #[test]
    fn test_skip_domain_name_standard() {
        // 7 "example" 3 "com" 0
        let data = vec![
            7, b'e', b'x', b'a', b'm', b'p', b'l', b'e', 3, b'c', b'o', b'm', 0,
        ];
        let offset = skip_domain_name(&data, 0).unwrap();
        assert_eq!(offset, 13);
    }

    #[test]
    fn test_skip_domain_name_pointer() {
        // Compression pointer: 0xC0 0x00 (pointer to offset 0).
        let data = vec![0xC0, 0x00];
        let offset = skip_domain_name(&data, 0).unwrap();
        assert_eq!(offset, 2);
    }

    #[test]
    fn test_skip_domain_name_empty() {
        // Just a zero-length terminator.
        let data = vec![0];
        let offset = skip_domain_name(&data, 0).unwrap();
        assert_eq!(offset, 1);
    }

    // ─── Answer Parsing ───

    #[test]
    fn test_parse_answer_a_record() {
        // Build a simple A record answer.
        let mut data = Vec::new();

        // Name: pointer to offset 0 (simplification).
        data.push(0xC0);
        data.push(0x00);

        // Type: A (1).
        data.extend_from_slice(&DNS_TYPE_A.to_be_bytes());

        // Class: IN (1).
        data.extend_from_slice(&DNS_CLASS_IN.to_be_bytes());

        // TTL: 300 seconds.
        data.extend_from_slice(&300u32.to_be_bytes());

        // RDLENGTH: 4 bytes.
        data.extend_from_slice(&4u16.to_be_bytes());

        // RDATA: 192.168.1.1.
        data.extend_from_slice(&[192, 168, 1, 1]);

        let (answer, offset) = parse_answer(&data, 0).unwrap();
        assert_eq!(answer.rtype, DNS_TYPE_A);
        assert_eq!(answer.rclass, DNS_CLASS_IN);
        assert_eq!(answer.ttl, 300);
        assert_eq!(answer.rdata, vec![192, 168, 1, 1]);
        assert_eq!(offset, data.len());
    }

    // ─── A Record Finding ───

    #[test]
    fn test_find_a_record() {
        let answers = vec![DnsAnswer {
            name: vec![0xC0, 0x00],
            rtype: DNS_TYPE_A,
            rclass: DNS_CLASS_IN,
            ttl: 300,
            rdata: vec![192, 168, 1, 1],
        }];

        let ip = find_a_record(&answers).unwrap();
        assert_eq!(ip, [192, 168, 1, 1]);
    }

    #[test]
    fn test_find_a_record_no_a_record() {
        let answers = vec![DnsAnswer {
            name: vec![0xC0, 0x00],
            rtype: 5, // CNAME
            rclass: DNS_CLASS_IN,
            ttl: 300,
            rdata: vec![7, b'e', b'x', b'a', b'm', b'p', b'l', b'e'],
        }];

        assert!(find_a_record(&answers).is_none());
    }

    #[test]
    fn test_find_a_record_multiple_answers() {
        let answers = vec![
            DnsAnswer {
                name: vec![0xC0, 0x00],
                rtype: 5, // CNAME
                rclass: DNS_CLASS_IN,
                ttl: 300,
                rdata: vec![7, b'e', b'x', b'a', b'm', b'p', b'l', b'e'],
            },
            DnsAnswer {
                name: vec![0xC0, 0x00],
                rtype: DNS_TYPE_A,
                rclass: DNS_CLASS_IN,
                ttl: 300,
                rdata: vec![10, 0, 0, 1],
            },
        ];

        let ip = find_a_record(&answers).unwrap();
        assert_eq!(ip, [10, 0, 0, 1]);
    }

    // ─── DNS Response Parsing ───

    #[test]
    fn test_parse_dns_response_valid() {
        // Build a minimal DNS response.
        let mut response = Vec::new();

        // Header.
        response.extend_from_slice(&DNS_TRANSACTION_ID.to_be_bytes()); // ID
        response.extend_from_slice(&0x8180u16.to_be_bytes()); // Flags (QR=1, RD=1, RA=1)
        response.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
        response.extend_from_slice(&1u16.to_be_bytes()); // ANCOUNT
        response.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
        response.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT

        // Question: example.com A IN.
        response.extend_from_slice(&[
            7, b'e', b'x', b'a', b'm', b'p', b'l', b'e', 3, b'c', b'o', b'm', 0,
        ]);
        response.extend_from_slice(&DNS_TYPE_A.to_be_bytes());
        response.extend_from_slice(&DNS_CLASS_IN.to_be_bytes());

        // Answer: pointer to question name.
        response.push(0xC0);
        response.push(0x0C); // Pointer to offset 12 (after header).
        response.extend_from_slice(&DNS_TYPE_A.to_be_bytes());
        response.extend_from_slice(&DNS_CLASS_IN.to_be_bytes());
        response.extend_from_slice(&300u32.to_be_bytes()); // TTL
        response.extend_from_slice(&4u16.to_be_bytes()); // RDLENGTH
        response.extend_from_slice(&[93, 184, 216, 34]); // 93.184.216.34

        let ip = parse_dns_response(&response).unwrap();
        assert_eq!(ip, [93, 184, 216, 34]);
    }

    #[test]
    fn test_parse_dns_response_id_mismatch() {
        let mut response = vec![0u8; DNS_HEADER_SIZE];
        write_u16_be(&mut response, 0, 0x1234); // Wrong ID.
        write_u16_be(&mut response, 2, 0x8180); // Flags (QR=1).

        assert!(parse_dns_response(&response).is_none());
    }

    #[test]
    fn test_parse_dns_response_not_response() {
        let mut response = vec![0u8; DNS_HEADER_SIZE];
        write_u16_be(&mut response, 0, DNS_TRANSACTION_ID);
        write_u16_be(&mut response, 2, 0x0100); // Flags (QR=0, RD=1).

        assert!(parse_dns_response(&response).is_none());
    }

    #[test]
    fn test_parse_dns_response_error() {
        let mut response = vec![0u8; DNS_HEADER_SIZE];
        write_u16_be(&mut response, 0, DNS_TRANSACTION_ID);
        write_u16_be(&mut response, 2, 0x8183); // Flags (QR=1, RD=1, RA=1, RCODE=3 = NXDOMAIN).

        assert!(parse_dns_response(&response).is_none());
    }

    #[test]
    fn test_parse_dns_response_no_answers() {
        let mut response = vec![0u8; DNS_HEADER_SIZE];
        write_u16_be(&mut response, 0, DNS_TRANSACTION_ID);
        write_u16_be(&mut response, 2, 0x8180); // Flags (QR=1, RD=1, RA=1).
        write_u16_be(&mut response, 4, 1); // QDCOUNT
        write_u16_be(&mut response, 6, 0); // ANCOUNT

        // Question.
        response.extend_from_slice(&[
            7, b'e', b'x', b'a', b'm', b'p', b'l', b'e', 3, b'c', b'o', b'm', 0,
        ]);
        response.extend_from_slice(&DNS_TYPE_A.to_be_bytes());
        response.extend_from_slice(&DNS_CLASS_IN.to_be_bytes());

        assert!(parse_dns_response(&response).is_none());
    }

    // ─── Byte Helpers ───

    #[test]
    fn test_read_u16_be() {
        let data = [0x12, 0x34];
        assert_eq!(read_u16_be(&data, 0), 0x1234);
    }

    #[test]
    fn test_read_u32_be() {
        let data = [0x01, 0x02, 0x03, 0x04];
        assert_eq!(read_u32_be(&data, 0), 0x01020304);
    }

    #[test]
    fn test_write_u16_be() {
        let mut buf = [0u8; 2];
        write_u16_be(&mut buf, 0, 0x1234);
        assert_eq!(buf, [0x12, 0x34]);
    }
}
