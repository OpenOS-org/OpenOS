//! User-space ARP + ICMP ping for OpenOS.
//!
//! Discovers the gateway MAC via ARP, then sends an ICMP echo request
//! (ping) to the QEMU gateway at 10.0.2.2 and waits for a reply.

#![no_std]
#![no_main]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::panic::PanicInfo;

use openos_sdk::{console, net, process};

/// Simple bump allocator for user-space (64 KiB heap).
struct BumpAllocator {
    heap: core::cell::UnsafeCell<[u8; 65536]>,
    offset: core::cell::Cell<usize>,
}

unsafe impl Sync for BumpAllocator {}

unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let align = layout.align();
        let size = layout.size();
        let mut off = self.offset.get();
        off = (off + align - 1) & !(align - 1);
        if off + size > 65536 {
            return core::ptr::null_mut();
        }
        let ptr = (*self.heap.get()).as_mut_ptr().add(off);
        self.offset.set(off + size);
        ptr
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // Bump allocator: no-op dealloc.
    }
}

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator {
    heap: core::cell::UnsafeCell::new([0u8; 65536]),
    offset: core::cell::Cell::new(0),
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Our assumed MAC address (QEMU default for virtio-net).
const OUR_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];

/// Our IP address (assigned by QEMU user-mode networking).
const OUR_IP: [u8; 4] = [10, 0, 2, 15];

/// Gateway IP (QEMU user-mode default).
const GATEWAY_IP: [u8; 4] = [10, 0, 2, 2];

/// Broadcast MAC.
const BROADCAST_MAC: [u8; 6] = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];

/// Ethernet type for ARP.
const ETHERTYPE_ARP: u16 = 0x0806;

/// Ethernet type for IPv4.
const ETHERTYPE_IPV4: u16 = 0x0800;

/// ARP hardware type: Ethernet.
const ARP_HTYPE_ETHERNET: u16 = 1;

/// ARP protocol type: IPv4.
const ARP_PTYPE_IPV4: u16 = 0x0800;

/// ARP operation: reply.
const ARP_OP_REPLY: u16 = 2;

/// ICMP type: echo request.
const ICMP_ECHO_REQUEST: u8 = 8;

/// ICMP type: echo reply.
const ICMP_ECHO_REPLY: u8 = 0;

/// Max receive attempts before giving up.
const MAX_RX_ATTEMPTS: usize = 10_000;

/// ICMP payload size (bytes).
const ICMP_DATA_LEN: usize = 32;

// ---------------------------------------------------------------------------
// Panic handler
// ---------------------------------------------------------------------------

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = console::writeln("PANIC in ping!");
    process::exit(1);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Format a usize as a decimal string into a static buffer.
fn format_usize(mut n: usize) -> &'static str {
    static mut BUF: [u8; 20] = [0u8; 20];
    if n == 0 {
        // SAFETY: single-threaded user program, no concurrent access.
        unsafe {
            BUF[19] = b'0';
            return core::str::from_utf8_unchecked(&BUF[19..20]);
        }
    }
    let mut i = 19;
    while n > 0 {
        // SAFETY: single-threaded user program, only ASCII digits written.
        unsafe {
            BUF[i] = b'0' + (n % 10) as u8;
        }
        n /= 10;
        i = i.saturating_sub(1);
    }
    // SAFETY: slice contains only ASCII digits.
    unsafe { core::str::from_utf8_unchecked(&BUF[i + 1..20]) }
}

/// Write a MAC address as "AA:BB:CC:DD:EE:FF".
fn write_mac(mac: &[u8; 6]) {
    static HEX: &[u8; 16] = b"0123456789ABCDEF";
    for (i, &byte) in mac.iter().enumerate() {
        if i > 0 {
            let _ = console::write(":");
        }
        // SAFETY: index 0..15 is always in bounds.
        let hi = unsafe { *HEX.get_unchecked((byte >> 4) as usize) } as char;
        let lo = unsafe { *HEX.get_unchecked((byte & 0x0F) as usize) } as char;
        let _ = console::write(
            // SAFETY: two ASCII hex digits.
            unsafe { core::str::from_utf8_unchecked(&[hi as u8, lo as u8]) },
        );
    }
}

/// Compute the Internet Checksum (RFC 1071) over a byte slice.
fn internet_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut chunks = data.chunks_exact(2);
    for chunk in &mut chunks {
        let word = u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
        sum = sum.wrapping_add(word);
    }
    let remainder = chunks.remainder();
    if !remainder.is_empty() {
        sum = sum.wrapping_add((remainder[0] as u32) << 8);
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

// ---------------------------------------------------------------------------
// Ethernet frame builder
// ---------------------------------------------------------------------------

/// Build a raw Ethernet frame in `buf`. Returns the frame length.
fn build_ethernet_frame(
    buf: &mut [u8],
    dst_mac: &[u8; 6],
    src_mac: &[u8; 6],
    ethertype: u16,
    payload: &[u8],
) -> usize {
    buf[0..6].copy_from_slice(dst_mac);
    buf[6..12].copy_from_slice(src_mac);
    buf[12] = (ethertype >> 8) as u8;
    buf[13] = (ethertype & 0xFF) as u8;
    let hdr_len = 14;
    buf[hdr_len..hdr_len + payload.len()].copy_from_slice(payload);
    let total = hdr_len + payload.len();
    // Pad to minimum Ethernet frame size (60 bytes, excluding FCS).
    if total < 60 {
        for b in &mut buf[total..60] {
            *b = 0;
        }
        60
    } else {
        total
    }
}

// ---------------------------------------------------------------------------
// ARP
// ---------------------------------------------------------------------------

/// Build an ARP request payload (28 bytes for Ethernet/IPv4).
fn build_arp_request(
    buf: &mut [u8],
    sender_mac: &[u8; 6],
    sender_ip: &[u8; 4],
    target_ip: &[u8; 4],
) {
    // Hardware type: Ethernet (0x0001)
    buf[0] = 0x00;
    buf[1] = 0x01;
    // Protocol type: IPv4 (0x0800)
    buf[2] = 0x08;
    buf[3] = 0x00;
    // Hardware size: 6
    buf[4] = 6;
    // Protocol size: 4
    buf[5] = 4;
    // Opcode: request (1)
    buf[6] = 0x00;
    buf[7] = 0x01;
    // Sender MAC
    buf[8..14].copy_from_slice(sender_mac);
    // Sender IP
    buf[14..18].copy_from_slice(sender_ip);
    // Target MAC: unknown (zeroes)
    buf[18..24].copy_from_slice(&[0u8; 6]);
    // Target IP
    buf[24..28].copy_from_slice(target_ip);
}

/// Check if an Ethernet frame is an ARP reply for our request.
/// Returns `true` and fills `sender_mac` with the responder's MAC.
fn parse_arp_reply(frame: &[u8], out_mac: &mut [u8; 6]) -> bool {
    if frame.len() < 42 {
        return false;
    }
    let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
    if ethertype != ETHERTYPE_ARP {
        return false;
    }
    let arp = &frame[14..];
    let htype = u16::from_be_bytes([arp[0], arp[1]]);
    let ptype = u16::from_be_bytes([arp[2], arp[3]]);
    let op = u16::from_be_bytes([arp[6], arp[7]]);
    if htype != ARP_HTYPE_ETHERNET || ptype != ARP_PTYPE_IPV4 || op != ARP_OP_REPLY {
        return false;
    }
    // Check that the reply is for our target IP.
    if arp[24..28] != OUR_IP {
        return false;
    }
    // Sender MAC is at ARP offset 8.
    out_mac.copy_from_slice(&arp[8..14]);
    true
}

/// Send an ARP request for the gateway and wait for a reply.
/// Returns the gateway's MAC on success.
fn arp_discover_gateway() -> Result<[u8; 6], &'static str> {
    let _ = console::writeln("ping: sending ARP request for 10.0.2.2 ...");

    // Build ARP request Ethernet frame.
    let mut arp_payload = [0u8; 28];
    build_arp_request(&mut arp_payload, &OUR_MAC, &OUR_IP, &GATEWAY_IP);

    let mut frame = [0u8; 1514];
    let frame_len = build_ethernet_frame(
        &mut frame,
        &BROADCAST_MAC,
        &OUR_MAC,
        ETHERTYPE_ARP,
        &arp_payload,
    );

    // Send the ARP request.
    match net::send_frame(&frame[..frame_len]) {
        Ok(bytes) => {
            let _ = console::write("ping: ARP request sent (");
            let _ = console::write(format_usize(bytes));
            let _ = console::writeln(" bytes)");
        }
        Err(_) => {
            let _ = console::writeln("ping: ARP send failed");
            return Err("ARP send failed");
        }
    }

    // Poll for the ARP reply.
    let mut rx_buf = [0u8; 1514];
    let mut gw_mac = [0u8; 6];

    for _attempt in 0..MAX_RX_ATTEMPTS {
        match net::receive_frame(&mut rx_buf) {
            Ok(len) => {
                if len >= 42 && parse_arp_reply(&rx_buf[..len], &mut gw_mac) {
                    let _ = console::write("ping: ARP reply from gateway MAC: ");
                    write_mac(&gw_mac);
                    let _ = console::writeln("");
                    return Ok(gw_mac);
                }
            }
            Err(openos_sdk::Error::WouldBlock) => {
                // No frame available yet, try again.
            }
            Err(_) => {
                return Err("ARP receive error");
            }
        }
    }

    Err("ARP timeout: no reply from gateway")
}

// ---------------------------------------------------------------------------
// ICMP
// ---------------------------------------------------------------------------

/// Build an ICMP echo request payload. Returns the ICMP packet bytes.
///
/// Layout: type(1) + code(1) + checksum(2) + id(2) + seq(2) + data(N)
fn build_icmp_echo_request(buf: &mut [u8], seq: u16) -> usize {
    let icmp_len = 8 + ICMP_DATA_LEN;
    // Type: echo request
    buf[0] = ICMP_ECHO_REQUEST;
    // Code: 0
    buf[1] = 0;
    // Checksum: placeholder (computed below)
    buf[2] = 0;
    buf[3] = 0;
    // Identifier
    buf[4] = 0x4F; // 'O'
    buf[5] = 0x53; // 'S'
                   // Sequence number
    buf[6] = (seq >> 8) as u8;
    buf[7] = (seq & 0xFF) as u8;
    // Payload: fill with repeating byte pattern.
    for i in 0..ICMP_DATA_LEN {
        buf[8 + i] = (i & 0xFF) as u8;
    }
    // Compute checksum over the full ICMP packet.
    let csum = internet_checksum(&buf[..icmp_len]);
    buf[2] = (csum >> 8) as u8;
    buf[3] = (csum & 0xFF) as u8;
    icmp_len
}

/// Build a minimal IPv4 header (20 bytes, no options) for an ICMP packet.
fn build_ipv4_header(buf: &mut [u8], total_len: u16, protocol: u8) {
    buf[0] = 0x45; // Version 4, IHL 5 (20 bytes)
    buf[1] = 0x00; // DSCP / ECN
    buf[2] = (total_len >> 8) as u8;
    buf[3] = (total_len & 0xFF) as u8;
    buf[4] = 0x00; // Identification (high)
    buf[5] = 0x00; // Identification (low)
    buf[6] = 0x40; // Flags: Don't Fragment
    buf[7] = 0x00; // Fragment offset
    buf[8] = 64; // TTL
    buf[9] = protocol;
    // Checksum: placeholder (computed below)
    buf[10] = 0;
    buf[11] = 0;
    // Source IP
    buf[12..16].copy_from_slice(&OUR_IP);
    // Destination IP
    buf[16..20].copy_from_slice(&GATEWAY_IP);
    // Compute IPv4 header checksum.
    let csum = internet_checksum(&buf[..20]);
    buf[10] = (csum >> 8) as u8;
    buf[11] = (csum & 0xFF) as u8;
}

/// Check if an Ethernet frame is an ICMP echo reply from the gateway.
fn parse_icmp_echo_reply(frame: &[u8]) -> bool {
    if frame.len() < 14 + 20 + 8 {
        return false;
    }
    // EtherType check.
    let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
    if ethertype != ETHERTYPE_IPV4 {
        return false;
    }
    let ipv4 = &frame[14..];
    // Must be IPv4, protocol ICMP (1).
    if ipv4[0] != 0x45 || ipv4[9] != 1 {
        return false;
    }
    // Source must be gateway.
    if ipv4[12..16] != GATEWAY_IP {
        return false;
    }
    // ICMP header starts at offset 20 from IPv4 header.
    let icmp = &ipv4[20..];
    // Type must be echo reply.
    if icmp[0] != ICMP_ECHO_REPLY {
        return false;
    }
    true
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let _ = console::writeln("ping: starting ARP + ICMP echo test");

    // Step 1: ARP to discover gateway MAC.
    let gw_mac = match arp_discover_gateway() {
        Ok(mac) => mac,
        Err(msg) => {
            let _ = console::write("ping: ");
            let _ = console::writeln(msg);
            let _ = console::writeln("PING FAILED");
            process::exit(1);
        }
    };

    // Step 2: Send ICMP echo request.
    let _ = console::writeln("ping: sending ICMP echo request to 10.0.2.2 ...");

    let mut icmp_buf = [0u8; 64];
    let icmp_len = build_icmp_echo_request(&mut icmp_buf, 1);

    // Build IPv4 packet: header (20) + ICMP payload.
    let ipv4_total_len: u16 = 20 + icmp_len as u16;
    let mut ipv4_buf = [0u8; 1500];
    build_ipv4_header(&mut ipv4_buf, ipv4_total_len, 1); // protocol 1 = ICMP
    ipv4_buf[20..20 + icmp_len].copy_from_slice(&icmp_buf[..icmp_len]);

    // Wrap in Ethernet frame.
    let mut frame = [0u8; 1514];
    let frame_len = build_ethernet_frame(
        &mut frame,
        &gw_mac,
        &OUR_MAC,
        ETHERTYPE_IPV4,
        &ipv4_buf[..ipv4_total_len as usize],
    );

    match net::send_frame(&frame[..frame_len]) {
        Ok(bytes) => {
            let _ = console::write("ping: ICMP echo request sent (");
            let _ = console::write(format_usize(bytes));
            let _ = console::writeln(" bytes)");
        }
        Err(_) => {
            let _ = console::writeln("ping: ICMP send failed");
            let _ = console::writeln("PING FAILED");
            process::exit(1);
        }
    }

    // Step 3: Wait for ICMP echo reply.
    let mut rx_buf = [0u8; 1514];
    for _attempt in 0..MAX_RX_ATTEMPTS {
        match net::receive_frame(&mut rx_buf) {
            Ok(len) => {
                if len >= 14 + 20 + 8 && parse_icmp_echo_reply(&rx_buf[..len]) {
                    let _ = console::writeln("ping: received ICMP echo reply from 10.0.2.2");
                    let _ = console::writeln("PING OK");
                    process::exit(0);
                }
            }
            Err(openos_sdk::Error::WouldBlock) => {
                // No frame available yet, keep polling.
            }
            Err(_) => {
                let _ = console::writeln("ping: ICMP receive error");
                let _ = console::writeln("PING FAILED");
                process::exit(1);
            }
        }
    }

    let _ = console::writeln("ping: ICMP echo reply timeout");
    let _ = console::writeln("PING FAILED");
    process::exit(1);
}
