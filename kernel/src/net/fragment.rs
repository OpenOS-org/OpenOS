//! IPv4 fragmentation and reassembly (RFC 791).
//!
//! ## Architecture
//!
//! ```text
//! net::handle_frame() -> fragment::try_reassemble()
//!     |
//!     +-- Is fragment? -> add to fragment cache; return None (not ready)
//!     +-- All fragments? -> reassemble into complete datagram; return Some(payload)
//!     +-- Not fragment? -> return None (pass-through, caller handles normally)
//! ```
//!
//! A background timer (`expire_fragments`) is called from the network service
//! loop to garbage-collect incomplete fragment sets older than the timeout.
//!
//! ## Fragment Cache
//!
//! Each fragment is keyed by the tuple `(src_ip, dst_ip, identification, protocol)`.
//! The cache stores all received fragments for a given datagram until either:
//! - All fragments arrive and the datagram is reassembled, or
//! - The fragment timeout expires (30 seconds).
//!
//! ## Fragment Header Fields (RFC 791 Section 3.1)
//!
//! ```text
//!  0                   1                   2                   3
//!  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |  Ver  | IHL  |       DS      |          Total Length         |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |        Identification         |Flags|    Fragment Offset     |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |   TTL  | Protocol |        Header Checksum                   |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                       Source Address                          |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                    Destination Address                        |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! ```
//!
//! ## Constants
//!
//! All numeric values are documented named constants — no magic numbers.

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

use spin::Mutex;

use crate::serial_println;

// ─────────────────── Constants ───────────────────

/// Fragment timeout in timer ticks (30 seconds at ~18.2 Hz timer).
const FRAGMENT_TIMEOUT_TICKS: u64 = 546;

/// Maximum number of concurrent fragment sets in the cache.
const MAX_FRAGMENT_SETS: usize = 256;

/// Maximum number of fragments per datagram.
const MAX_FRAGMENTS_PER_DATAGRAM: usize = 64;

/// Minimum fragment data size: 8 bytes (offset unit).
const FRAGMENT_OFFSET_UNIT: usize = 8;

/// IPv4 header minimum size (20 bytes, no options).
const IP_HEADER_MIN_SIZE: usize = 20;

// ─────────────────── Fragment Flags (RFC 791) ───────────────────

/// Bitmask for the More-Fragments (MF) flag in the flags/offset field.
const MF_FLAG: u16 = 0x2000;

/// Bitmask for the Don't-Fragment (DF) flag.
#[allow(dead_code)]
const DF_FLAG: u16 = 0x4000;

/// Bitmask for the fragment offset field (lower 13 bits of flag/offset word).
const OFFSET_MASK: u16 = 0x1FFF;

// ─────────────────── Structures ───────────────────

/// Key for identifying a specific IP datagram fragment set.
///
/// Fragment reassembly groups fragments by (src, dst, identification, protocol)
/// as specified in RFC 791 Section 3.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct FragmentKey {
    /// Source IPv4 address (network byte order).
    src_ip: u32,
    /// Destination IPv4 address (network byte order).
    dst_ip: u32,
    /// IP identification field (16 bits).
    identification: u16,
    /// Protocol number (e.g. 6 for TCP, 17 for UDP).
    protocol: u8,
}

/// A single received fragment with metadata.
#[derive(Debug, Clone)]
struct FragmentData {
    /// Fragment offset in bytes (converted from 8-byte units).
    offset: usize,
    /// Payload data (IP header already stripped, includes only fragment body).
    data: Vec<u8>,
    /// Whether this fragment has the More-Fragments flag set.
    more_fragments: bool,
}

/// Tracks the reassembly state for one IP datagram.
#[derive(Debug, Clone)]
struct FragmentSet {
    /// Receipt timestamp (tick count) for timeout detection.
    timestamp: u64,
    /// Collected fragments.
    fragments: Vec<FragmentData>,
    /// Total expected length of the reassembled payload (known only after
    /// the last fragment arrives, when `more_fragments` is false for a fragment).
    total_payload_length: Option<usize>,
}

/// Global fragment reassembly cache.
///
/// Maps a `FragmentKey` (src, dst, id, protocol) to the set of fragments
/// received so far for that datagram. Protected by a spinlock since the
/// network service loop (which inserts fragments) and the expiry timer
/// (which removes old sets) run on different contexts.
static FRAGMENT_CACHE: Mutex<BTreeMap<FragmentKey, FragmentSet>> = Mutex::new(BTreeMap::new());

/// Fragment reassembly statistics for diagnostics.
#[derive(Debug, Clone, Copy, Default)]
pub struct FragmentStats {
    /// Total fragments received.
    pub fragments_received: u64,
    /// Complete datagrams successfully reassembled.
    pub datagrams_reassembled: u64,
    /// Fragment sets that timed out waiting for more fragments.
    pub timed_out: u64,
    /// Fragments rejected due to overlap errors.
    pub overlap_errors: u64,
    /// Fragments rejected because the cache was full.
    pub cache_full_drops: u64,
    /// Fragment sets rejected because they reached max fragments.
    pub too_many_fragments: u64,
    /// Fragments with invalid offset or length.
    pub invalid_fragments: u64,
}

/// Default fragment statistics value for static initialization.
const FRAGMENT_STATS_DEFAULT: FragmentStats = FragmentStats {
    fragments_received: 0,
    datagrams_reassembled: 0,
    timed_out: 0,
    overlap_errors: 0,
    cache_full_drops: 0,
    too_many_fragments: 0,
    invalid_fragments: 0,
};

/// Global fragment statistics.
static FRAGMENT_STATS: Mutex<FragmentStats> = Mutex::new(FRAGMENT_STATS_DEFAULT);

// ─────────────────── Public API ───────────────────

/// Get a snapshot of fragment reassembly statistics.
#[must_use]
pub fn get_fragment_stats() -> FragmentStats {
    *FRAGMENT_STATS.lock()
}

/// Parse fragment information from an IPv4 header.
///
/// Extracts the identification, flags, and fragment offset from a raw IPv4
/// header. Returns `None` if the header is too short.
///
/// # Arguments
/// - `data`: Raw IPv4 packet (including header).
///
/// # Returns
/// A tuple of (`identification`, `is_fragment`, `more_fragments`, `offset_in_bytes`)
/// where `is_fragment` is true if DF is not set and (MF is set or offset > 0).
#[must_use]
pub fn parse_fragment_info(data: &[u8]) -> Option<(u16, bool, bool, usize)> {
    if data.len() < IP_HEADER_MIN_SIZE {
        return None;
    }

    let identification = u16::from_be_bytes([data[4], data[5]]);
    let flags_offset = u16::from_be_bytes([data[6], data[7]]);

    let more_fragments = (flags_offset & MF_FLAG) != 0;
    let df_set = (flags_offset & DF_FLAG) != 0;
    let offset_bytes = (usize::from(flags_offset & OFFSET_MASK)) * FRAGMENT_OFFSET_UNIT;

    // A packet is a fragment if DF is not set AND (MF is set or offset != 0).
    // A packet with MF=0 and offset=0 is the last (or only) fragment.
    let is_fragment = !df_set && (more_fragments || offset_bytes > 0);

    Some((identification, is_fragment, more_fragments, offset_bytes))
}

/// Attempt to reassemble an incoming IPv4 packet.
///
/// If the packet is not a fragment, returns `None` (caller should handle
/// the packet normally). If it is a fragment, this function buffers it and
/// returns:
/// - `None` if more fragments are still needed.
/// - `Some(reassembled_payload)` if all fragments have arrived and the
///   datagram has been successfully reassembled.
///
/// # Arguments
/// - `data`: Raw IPv4 packet (including header).
/// - `now`: Current tick count for timestamping.
///
/// # Returns
/// - `Ok(None)`: Packet is not a fragment, or is a fragment but not yet complete.
/// - `Ok(Some(payload))`: Reassembled IP payload (header stripped, fragments merged).
/// - `Err(&'static str)`: Reassembly error (overlap, invalid, etc.).
pub fn try_reassemble(data: &[u8], now: u64) -> Result<Option<Vec<u8>>, &'static str> {
    if data.len() < IP_HEADER_MIN_SIZE {
        return Err("fragment: packet too short for IPv4 header");
    }

    let (identification, is_fragment, more_fragments, offset_bytes) =
        parse_fragment_info(data).ok_or("fragment: failed to parse fragment info")?;

    if !is_fragment {
        // Not a fragment — pass through.
        return Ok(None);
    }

    // Extract fragment metadata.
    let src_ip = u32::from_be_bytes([data[12], data[13], data[14], data[15]]);
    let dst_ip = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let protocol = data[9];
    let total_len = u16::from_be_bytes([data[2], data[3]]);
    let header_len = usize::from(data[0] & 0x0F) * 4;

    if usize::from(total_len) < header_len {
        return Err("fragment: total length less than header length");
    }

    // Extract the fragment payload (header stripped).
    let frag_payload = &data[header_len..usize::from(total_len)];

    if frag_payload.len() % FRAGMENT_OFFSET_UNIT != 0 && more_fragments {
        // Intermediate fragments must have payload length a multiple of 8.
        // The last fragment can have any length.
        FRAGMENT_STATS.lock().invalid_fragments += 1;
        return Err("fragment: intermediate fragment payload not aligned to 8 bytes");
    }

    let fragment = FragmentData {
        offset: offset_bytes,
        data: frag_payload.to_vec(),
        more_fragments,
    };

    let key = FragmentKey {
        src_ip,
        dst_ip,
        identification,
        protocol,
    };

    reassemble_fragment(key, fragment, now)
}

/// Expire fragment sets that have exceeded the timeout.
///
/// Called periodically from the network service loop to prevent stale
/// fragment sets from accumulating in the cache.
pub fn expire_fragments(now: u64) {
    let mut cache = FRAGMENT_CACHE.lock();
    let before = cache.len();
    cache.retain(|_key, set| now.saturating_sub(set.timestamp) < FRAGMENT_TIMEOUT_TICKS);
    let expired = before - cache.len();
    if expired > 0 {
        FRAGMENT_STATS.lock().timed_out += expired as u64;
        serial_println!("[FRAG] Expired {} incomplete fragment set(s)", expired);
    }
}

/// Remove all entries from the fragment cache.
///
/// Used mainly for testing to ensure clean state between test cases.
pub fn clear_fragment_cache() {
    FRAGMENT_CACHE.lock().clear();
}

/// Reset fragment statistics to zero.
///
/// Used mainly for testing to ensure a clean stats state between test cases.
pub fn reset_fragment_stats() {
    *FRAGMENT_STATS.lock() = FRAGMENT_STATS_DEFAULT;
}

// ─────────────────── Internal functions ───────────────────

/// Core reassembly logic: insert a fragment into the cache and check
/// if the datagram is complete.
///
/// # Arguments
/// - `key`: The fragment set key.
/// - `fragment`: The new fragment to add.
/// - `now`: Current tick count.
///
/// # Returns
/// - `Ok(None)` if more fragments are needed.
/// - `Ok(Some(payload))` if the datagram is now complete.
/// - `Err(&'static str)` on error.
fn reassemble_fragment(
    key: FragmentKey,
    fragment: FragmentData,
    now: u64,
) -> Result<Option<Vec<u8>>, &'static str> {
    let mut cache = FRAGMENT_CACHE.lock();

    // Enforce maximum concurrent fragment sets.
    if !cache.contains_key(&key) && cache.len() >= MAX_FRAGMENT_SETS {
        FRAGMENT_STATS.lock().cache_full_drops += 1;
        return Err("fragment: cache full, dropping fragment");
    }

    let set = cache.entry(key).or_insert_with(|| FragmentSet {
        timestamp: now,
        fragments: Vec::new(),
        total_payload_length: None,
    });

    // Update timestamp on each fragment receipt to prevent premature expiry.
    set.timestamp = now;

    // Enforce maximum fragments per datagram.
    if set.fragments.len() >= MAX_FRAGMENTS_PER_DATAGRAM {
        FRAGMENT_STATS.lock().too_many_fragments += 1;
        return Err("fragment: too many fragments for datagram");
    }

    // Check for overlap with existing fragments.
    if has_overlap(&set.fragments, &fragment) {
        FRAGMENT_STATS.lock().overlap_errors += 1;
        return Err("fragment: overlapping fragment rejected");
    }

    // Insert the fragment, maintaining sorted order by offset.
    let insert_idx = set
        .fragments
        .binary_search_by(|f| f.offset.cmp(&fragment.offset))
        .unwrap_or_else(|idx| idx);
    set.fragments.insert(insert_idx, fragment);

    FRAGMENT_STATS.lock().fragments_received += 1;

    // Check if the datagram is complete.
    if let Some(payload) = check_complete(set) {
        // Remove the completed set from the cache.
        cache.remove(&key);
        FRAGMENT_STATS.lock().datagrams_reassembled += 1;
        serial_println!(
            "[FRAG] Reassembled datagram id={:#06x} src={:?} dst={:?} proto={} ({} bytes)",
            key.identification,
            super::format_ip(key.src_ip),
            super::format_ip(key.dst_ip),
            key.protocol,
            payload.len()
        );
        return Ok(Some(payload));
    }

    Ok(None)
}

/// Check if a new fragment overlaps any existing fragment.
///
/// Uses the RFC 791 overlap rule: fragments may be duplicate or fully
/// contained, but partial overlap that is not a simple duplicate or a
/// contained subset is rejected.
#[must_use]
fn has_overlap(existing: &[FragmentData], new: &FragmentData) -> bool {
    let new_start = new.offset;
    let new_end = new.offset + new.data.len();

    for f in existing {
        let exist_start = f.offset;
        let exist_end = f.offset + f.data.len();

        // Check for any overlap between the two ranges.
        if new_start < exist_end && exist_start < new_end {
            // If the new fragment is a duplicate (same offset, same data),
            // or is fully contained within an existing fragment, allow it.
            // If the existing fragment is fully contained within the new one,
            // that is also allowed (the new one supersedes).
            // Only reject true partial overlaps.
            let new_contained = new_start >= exist_start && new_end <= exist_end;
            let exist_contained = exist_start >= new_start && exist_end <= new_end;
            if !new_contained && !exist_contained {
                return true;
            }
        }
    }

    false
}

/// Check whether a complete datagram has been received and reassemble it.
///
/// A datagram is complete when:
/// 1. All fragments from offset 0 to the last byte are present.
/// 2. The last fragment (the one with `more_fragments = false`) has been
///    received, establishing the total payload length.
///
/// Returns `Some(reassembled_payload)` if complete, `None` otherwise.
#[must_use]
fn check_complete(set: &FragmentSet) -> Option<Vec<u8>> {
    // Find the last fragment (MF = 0) to determine the total length.
    let mut last_end: Option<usize> = None;

    for f in &set.fragments {
        if !f.more_fragments {
            last_end = Some(f.offset + f.data.len());
        }
    }

    // We need the last fragment to know the total length.
    let total_length = last_end?;

    // Verify that we have all fragments: the set must cover [0, total_length)
    // without gaps.
    // Sort by offset (they should already be sorted, but be safe).
    let mut sorted: Vec<&FragmentData> = set.fragments.iter().collect();
    sorted.sort_by_key(|f| f.offset);

    let mut covered_up_to: usize = 0;
    for f in sorted {
        if f.offset > covered_up_to {
            // Gap detected.
            return None;
        }
        covered_up_to = covered_up_to.max(f.offset + f.data.len());
    }

    if covered_up_to < total_length {
        // Trailing gap after the last fragment.
        return None;
    }

    // All fragments present — reassemble.
    let mut payload = vec![0u8; total_length];

    for f in &set.fragments {
        let end = (f.offset + f.data.len()).min(total_length);
        payload[f.offset..end].copy_from_slice(&f.data[..(end - f.offset)]);
    }

    Some(payload)
}

// ─────────────────── Fragment sending support ───────────────────

/// Fragment an IP payload into multiple IPv4 packets for transmission.
///
/// Given a complete IP payload and the IP header fields, produces a list of
/// complete Ethernet frames, each containing an IPv4 fragment.
///
/// # Arguments
/// - `payload`: The complete IP payload to fragment.
/// - `src_ip`: Source IPv4 address (network byte order).
/// - `dst_ip`: Destination IPv4 address (network byte order).
/// - `identification`: IP identification field (must be unique per {src,dst} pair).
/// - `protocol`: IP protocol number.
/// - `mtu`: Maximum Transmission Unit (including IP header).
///
/// # Returns
/// A vector of complete IPv4 packet payloads (IP header + fragment data).
/// Each element in the vector is an entire IP packet ready for further
/// encapsulation inside an Ethernet frame.
#[must_use]
pub fn fragment_payload(
    payload: &[u8],
    src_ip: u32,
    dst_ip: u32,
    identification: u16,
    protocol: u8,
    mtu: usize,
) -> Vec<Vec<u8>> {
    if payload.len() + IP_HEADER_MIN_SIZE <= mtu {
        // No fragmentation needed.
        let mut packet = vec![0u8; IP_HEADER_MIN_SIZE + payload.len()];
        build_ip_header(
            &mut packet,
            (IP_HEADER_MIN_SIZE + payload.len()) as u16,
            identification,
            0,
            false,
            protocol,
            src_ip,
            dst_ip,
        );
        packet[IP_HEADER_MIN_SIZE..].copy_from_slice(payload);
        // Compute IP header checksum.
        let checksum = super::internet_checksum(&packet[..IP_HEADER_MIN_SIZE]);
        packet[10] = (checksum >> 8) as u8;
        packet[11] = (checksum & 0xFF) as u8;
        return vec![packet];
    }

    // Calculate fragment sizes.
    // Each fragment must have an 8-byte aligned offset.
    let max_frag_data = ((mtu - IP_HEADER_MIN_SIZE) / FRAGMENT_OFFSET_UNIT) * FRAGMENT_OFFSET_UNIT;
    let num_fragments = payload.len().div_ceil(max_frag_data);

    let mut fragments = Vec::with_capacity(num_fragments);

    for i in 0..num_fragments {
        let offset = i * max_frag_data;
        let is_last = i == num_fragments - 1;
        let frag_size = if is_last {
            payload.len() - offset
        } else {
            max_frag_data
        };

        let total_len = IP_HEADER_MIN_SIZE + frag_size;
        let mut packet = vec![0u8; total_len];

        let fragment_offset_units = offset / FRAGMENT_OFFSET_UNIT;
        // Safety: cast is safe because max total length is bounded by MTU (usually 1500).
        build_ip_header(
            &mut packet,
            total_len as u16,
            identification,
            fragment_offset_units as u16,
            !is_last, // more_fragments = true for all but the last
            protocol,
            src_ip,
            dst_ip,
        );

        packet[IP_HEADER_MIN_SIZE..].copy_from_slice(&payload[offset..offset + frag_size]);

        // Recalculate IP header checksum.
        let checksum = super::internet_checksum(&packet[..IP_HEADER_MIN_SIZE]);
        packet[10] = (checksum >> 8) as u8;
        packet[11] = (checksum & 0xFF) as u8;

        fragments.push(packet);
    }

    fragments
}

/// Build an IPv4 header into the beginning of a buffer.
///
/// # Arguments
/// - `buf`: Buffer to write the header into (must be at least `IP_HEADER_MIN_SIZE`).
/// - `total_len`: Total IP packet length (header + payload).
/// - `identification`: IP identification field.
/// - `fragment_offset_units`: Fragment offset in 8-byte units.
/// - `more_fragments`: Whether the More-Fragments flag should be set.
/// - `protocol`: IP protocol number.
/// - `src_ip`: Source IPv4 address (network byte order).
/// - `dst_ip`: Destination IPv4 address (network byte order).
fn build_ip_header(
    buf: &mut [u8],
    total_len: u16,
    identification: u16,
    fragment_offset_units: u16,
    more_fragments: bool,
    protocol: u8,
    src_ip: u32,
    dst_ip: u32,
) {
    buf[0] = 0x45; // Version 4, IHL 5 (20 bytes)
    buf[1] = 0; // DSCP + ECN
    buf[2..4].copy_from_slice(&total_len.to_be_bytes());
    buf[4..6].copy_from_slice(&identification.to_be_bytes());

    let mut flags_offset = fragment_offset_units & OFFSET_MASK;
    if more_fragments {
        flags_offset |= MF_FLAG;
    }
    buf[6..8].copy_from_slice(&flags_offset.to_be_bytes());

    buf[8] = IP_DEFAULT_TTL; // TTL
    buf[9] = protocol;
    // Checksum — set to 0 initially, caller recalculates.
    buf[10] = 0;
    buf[11] = 0;
    buf[12..16].copy_from_slice(&src_ip.to_be_bytes());
    buf[16..20].copy_from_slice(&dst_ip.to_be_bytes());
}

/// Default TTL for generated IP headers.
const IP_DEFAULT_TTL: u8 = 64;

// ─────────────────── Tests ───────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create a minimal valid IPv4 fragment packet for testing.
    ///
    /// # Arguments
    /// - `identification`: IP identification.
    /// - `offset_bytes`: Fragment offset in bytes.
    /// - `more_fragments`: MF flag.
    /// - `payload`: Fragment body payload.
    /// - `src_ip`: Source IP (big-endian u32).
    /// - `dst_ip`: Destination IP (big-endian u32).
    /// - `protocol`: IP protocol number.
    fn make_fragment(
        identification: u16,
        offset_bytes: usize,
        more_fragments: bool,
        payload: &[u8],
        src_ip: u32,
        dst_ip: u32,
        protocol: u8,
    ) -> Vec<u8> {
        let total_len = IP_HEADER_MIN_SIZE + payload.len();
        let mut packet = vec![0u8; total_len];
        packet[0] = 0x45; // Version 4, IHL 5
        packet[2..4].copy_from_slice(&(total_len as u16).to_be_bytes());
        packet[4..6].copy_from_slice(&identification.to_be_bytes());

        let offset_units = (offset_bytes / FRAGMENT_OFFSET_UNIT) as u16;
        let mut flags_offset = offset_units & OFFSET_MASK;
        if more_fragments {
            flags_offset |= MF_FLAG;
        }
        packet[6..8].copy_from_slice(&flags_offset.to_be_bytes());

        packet[8] = 64; // TTL
        packet[9] = protocol;
        // Checksum left as 0 (not validated by our parser).
        packet[12..16].copy_from_slice(&src_ip.to_be_bytes());
        packet[16..20].copy_from_slice(&dst_ip.to_be_bytes());
        packet[IP_HEADER_MIN_SIZE..].copy_from_slice(payload);
        packet
    }

    #[test]
    fn test_parse_fragment_info_non_fragment() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();

        // A non-fragment packet: DF=0, MF=0, offset=0.
        let mut data = vec![0u8; IP_HEADER_MIN_SIZE];
        data[0] = 0x45;
        data[4..6].copy_from_slice(&0x1234u16.to_be_bytes());
        // Flags/offset: all zero — not a fragment.
        data[6..8].copy_from_slice(&0x0000u16.to_be_bytes());

        let (id, is_frag, mf, offset) = parse_fragment_info(&data).unwrap();
        assert_eq!(id, 0x1234);
        assert!(!is_frag);
        assert!(!mf);
        assert_eq!(offset, 0);
    }

    #[test]
    fn test_parse_fragment_info_with_mf() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();

        let mut data = vec![0u8; IP_HEADER_MIN_SIZE];
        data[0] = 0x45;
        data[4..6].copy_from_slice(&0xABCDu16.to_be_bytes());
        // MF flag set, offset = 0: this is a fragment (first fragment).
        let flags_offset = MF_FLAG;
        data[6..8].copy_from_slice(&flags_offset.to_be_bytes());

        let (id, is_frag, mf, offset) = parse_fragment_info(&data).unwrap();
        assert_eq!(id, 0xABCD);
        assert!(is_frag);
        assert!(mf);
        assert_eq!(offset, 0);
    }

    #[test]
    fn test_parse_fragment_info_with_offset() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();

        let mut data = vec![0u8; IP_HEADER_MIN_SIZE];
        data[0] = 0x45;
        // Offset = 40 bytes (5 units), MF = 0: last fragment at offset 40.
        let flags_offset = 5u16 & OFFSET_MASK;
        data[6..8].copy_from_slice(&flags_offset.to_be_bytes());

        let (id, is_frag, mf, offset) = parse_fragment_info(&data).unwrap();
        assert!(is_frag);
        assert!(!mf);
        assert_eq!(offset, 40);
    }

    #[test]
    fn test_parse_fragment_info_df_set() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();

        // DF=1, MF=0, offset=0: NOT a fragment (DF prevents fragmentation).
        let mut data = vec![0u8; IP_HEADER_MIN_SIZE];
        data[0] = 0x45;
        data[6..8].copy_from_slice(&DF_FLAG.to_be_bytes());

        let (_id, is_frag, _mf, _offset) = parse_fragment_info(&data).unwrap();
        assert!(!is_frag, "DF=1 should not be treated as a fragment");
    }

    #[test]
    fn test_parse_fragment_info_too_short() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        assert!(parse_fragment_info(&[0u8; 10]).is_none());
    }

    #[test]
    fn test_has_overlap_no_overlap() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();

        let existing = vec![FragmentData {
            offset: 0,
            data: vec![0u8; 8],
            more_fragments: true,
        }];

        let new = FragmentData {
            offset: 8,
            data: vec![0u8; 8],
            more_fragments: true,
        };

        assert!(!has_overlap(&existing, &new));
    }

    #[test]
    fn test_has_overlap_exact_overlap_allowed() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();

        // Exact duplicate: allowed (same offset, same size).
        let existing = vec![FragmentData {
            offset: 0,
            data: vec![0xAAu8; 8],
            more_fragments: true,
        }];

        let new = FragmentData {
            offset: 0,
            data: vec![0xAAu8; 8],
            more_fragments: true,
        };

        assert!(
            !has_overlap(&existing, &new),
            "exact duplicate should be allowed"
        );
    }

    #[test]
    fn test_has_overlap_contained_allowed() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();

        // New fragment is fully contained within an existing fragment.
        let existing = vec![FragmentData {
            offset: 0,
            data: vec![0xBBu8; 16],
            more_fragments: true,
        }];

        let new = FragmentData {
            offset: 4,
            data: vec![0xCCu8; 8],
            more_fragments: true,
        };

        assert!(
            !has_overlap(&existing, &new),
            "contained fragment should be allowed"
        );
    }

    #[test]
    fn test_has_overlap_partial_rejected() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();

        // Partial overlap: new starts at offset 4 (within existing 0-15)
        // and extends past existing to offset 20.
        let existing = vec![FragmentData {
            offset: 0,
            data: vec![0xDDu8; 16],
            more_fragments: true,
        }];

        let new = FragmentData {
            offset: 12,
            data: vec![0xEEu8; 16],
            more_fragments: true,
        };

        assert!(
            has_overlap(&existing, &new),
            "partial overlap should be rejected"
        );
    }

    #[test]
    fn test_check_complete_not_enough_fragments() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();

        let set = FragmentSet {
            timestamp: 0,
            fragments: vec![FragmentData {
                offset: 0,
                data: vec![0u8; 8],
                more_fragments: true,
            }],
            total_payload_length: None,
        };

        assert!(
            check_complete(&set).is_none(),
            "should not be complete with only one fragment that has MF=1"
        );
    }

    #[test]
    fn test_check_complete_single_last_fragment() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();

        let payload = b"Hello world";
        let set = FragmentSet {
            timestamp: 0,
            fragments: vec![FragmentData {
                offset: 0,
                data: payload.to_vec(),
                more_fragments: false,
            }],
            total_payload_length: None,
        };

        let result = check_complete(&set);
        assert!(
            result.is_some(),
            "single fragment with MF=0 should be complete"
        );
        assert_eq!(result.unwrap(), payload);
    }

    #[test]
    fn test_check_complete_two_fragments() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();

        let part1 = b"Hello ";
        let part2 = b"world!";

        let set = FragmentSet {
            timestamp: 0,
            fragments: vec![
                FragmentData {
                    offset: 0,
                    data: part1.to_vec(),
                    more_fragments: true,
                },
                FragmentData {
                    offset: 6,
                    data: part2.to_vec(),
                    more_fragments: false,
                },
            ],
            total_payload_length: None,
        };

        let result = check_complete(&set);
        assert!(result.is_some(), "two fragments should be complete");
        assert_eq!(result.unwrap(), b"Hello world!");
    }

    #[test]
    fn test_check_complete_gap_detected() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();

        let set = FragmentSet {
            timestamp: 0,
            fragments: vec![
                FragmentData {
                    offset: 0,
                    data: vec![0u8; 8],
                    more_fragments: true,
                },
                FragmentData {
                    offset: 16,
                    data: vec![0xFFu8; 8],
                    more_fragments: false,
                },
            ],
            total_payload_length: None,
        };

        // Gap between byte 8 and 16.
        assert!(check_complete(&set).is_none(), "gap should be detected");
    }

    #[test]
    fn test_full_reassembly_three_fragments() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();

        let src_ip: u32 = 0x0A00020F; // 10.0.2.15
        let dst_ip: u32 = 0x0100A8C0; // 192.168.0.1
        let id: u16 = 0x4321;
        let protocol: u8 = 6; // TCP

        // Create a 64-byte payload fragmented into 3 pieces: 24, 24, 16 bytes.
        let full_payload: Vec<u8> = (0..64).map(|i| i as u8).collect();

        let frag1 = &full_payload[0..24];
        let frag2 = &full_payload[24..48];
        let frag3 = &full_payload[48..64];

        let pkt1 = make_fragment(id, 0, true, frag1, src_ip, dst_ip, protocol);
        let pkt2 = make_fragment(id, 24, true, frag2, src_ip, dst_ip, protocol);
        let pkt3 = make_fragment(id, 48, false, frag3, src_ip, dst_ip, protocol);

        let now = 1000;

        // Submit fragments in non-sequential order.
        let r1 = try_reassemble(&pkt2, now).unwrap();
        assert!(r1.is_none(), "frag2 should not complete the set");

        let r2 = try_reassemble(&pkt3, now).unwrap();
        assert!(
            r2.is_none(),
            "frag3 should not complete the set (missing frag1)"
        );

        let r3 = try_reassemble(&pkt1, now).unwrap();
        assert!(r3.is_some(), "frag1 should complete the set");
        assert_eq!(r3.unwrap(), full_payload);

        clear_fragment_cache();
    }

    #[test]
    fn test_reassembly_out_of_order() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();

        let src_ip: u32 = 0x0A00020F;
        let dst_ip: u32 = 0x0100A8C0;
        let id: u16 = 0x1111;
        let protocol: u8 = 17; // UDP

        let full_payload: Vec<u8> = (0..40).map(|i| i as u8).collect();

        let frag1 = &full_payload[0..16];
        let frag2 = &full_payload[16..32];
        let frag3 = &full_payload[32..40];

        // Last fragment first, then first, then middle.
        let pkt3 = make_fragment(id, 32, false, frag3, src_ip, dst_ip, protocol);
        let pkt1 = make_fragment(id, 0, true, frag1, src_ip, dst_ip, protocol);
        let pkt2 = make_fragment(id, 16, true, frag2, src_ip, dst_ip, protocol);

        let now = 2000;

        let r = try_reassemble(&pkt3, now).unwrap();
        assert!(r.is_none());

        let r = try_reassemble(&pkt1, now).unwrap();
        assert!(r.is_none());

        let r = try_reassemble(&pkt2, now).unwrap();
        assert!(r.is_some());
        assert_eq!(r.unwrap(), full_payload);

        clear_fragment_cache();
    }

    #[test]
    fn test_non_fragment_passthrough() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();

        // A non-fragmented packet (DF=0, MF=0, offset=0).
        let mut data = vec![0u8; IP_HEADER_MIN_SIZE + 4];
        data[0] = 0x45;
        data[2..4].copy_from_slice(&(24u16).to_be_bytes());
        data[4..6].copy_from_slice(&0x7777u16.to_be_bytes());
        data[6..8].copy_from_slice(&0x0000u16.to_be_bytes());
        data[8] = 64;
        data[9] = 6; // TCP
        data[12..16].copy_from_slice(&0x0A00020Fu32.to_be_bytes());
        data[16..20].copy_from_slice(&0x0100A8C0u32.to_be_bytes());
        data[20..24].copy_from_slice(b"test");

        let r = try_reassemble(&data, 3000).unwrap();
        assert!(r.is_none(), "non-fragment should return None");

        clear_fragment_cache();
    }

    #[test]
    fn test_fragment_timeout() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();

        let src_ip: u32 = 0x0A00020F;
        let dst_ip: u32 = 0x0100A8C0;
        let id: u16 = 0xDEAD;
        let protocol: u8 = 6;

        let pkt = make_fragment(id, 0, true, &[0u8; 8], src_ip, dst_ip, protocol);

        let r = try_reassemble(&pkt, 100).unwrap();
        assert!(r.is_none());

        // Expire at a time far in the future — should remove the set.
        expire_fragments(FRAGMENT_TIMEOUT_TICKS + 200);

        // Cache should be empty now.
        assert!(FRAGMENT_CACHE.lock().is_empty());

        clear_fragment_cache();
    }

    #[test]
    fn test_expire_fragments_no_removal_before_timeout() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();

        let src_ip: u32 = 0x0A00020F;
        let dst_ip: u32 = 0x0100A8C0;
        let id: u16 = 0xBEEF;
        let protocol: u8 = 17;

        let pkt = make_fragment(id, 0, true, &[0xFFu8; 8], src_ip, dst_ip, protocol);
        let r = try_reassemble(&pkt, 5000).unwrap();
        assert!(r.is_none());

        // Expire with a time just before the timeout would trigger.
        expire_fragments(5000 + FRAGMENT_TIMEOUT_TICKS - 1);

        assert!(
            !FRAGMENT_CACHE.lock().is_empty(),
            "set should not expire before timeout"
        );

        clear_fragment_cache();
    }

    #[test]
    fn test_overlap_rejected() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();

        let src_ip: u32 = 0x0A00020F;
        let dst_ip: u32 = 0x0100A8C0;
        let id: u16 = 0xCAFE;
        let protocol: u8 = 6;

        let pkt1 = make_fragment(id, 0, true, &[0u8; 16], src_ip, dst_ip, protocol);
        let r = try_reassemble(&pkt1, 100).unwrap();
        assert!(r.is_none());

        // Partial overlap with existing fragment.
        let pkt2 = make_fragment(id, 8, true, &[0xFFu8; 16], src_ip, dst_ip, protocol);
        let r = try_reassemble(&pkt2, 100);
        assert!(r.is_err(), "partial overlap should be rejected");
        assert_eq!(r.unwrap_err(), "fragment: overlapping fragment rejected");

        clear_fragment_cache();
    }

    #[test]
    fn test_fragment_payload_single() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();

        // Payload smaller than MTU — should produce a single fragment.
        let payload = b"Small payload";
        let mtu = 1500;
        let fragments = fragment_payload(payload, 0x0A00020F, 0x0100A8C0, 0x1234, 6, mtu);

        assert_eq!(
            fragments.len(),
            1,
            "payload under MTU should produce 1 fragment"
        );
        let pkt = &fragments[0];
        // Check it's a valid IP header.
        assert_eq!(pkt[0], 0x45);
        // Check correct total length.
        let total_len = u16::from_be_bytes([pkt[2], pkt[3]]);
        assert_eq!(total_len as usize, IP_HEADER_MIN_SIZE + payload.len());
        // Check non-fragmented (MF=0, offset=0).
        let flags_offset = u16::from_be_bytes([pkt[6], pkt[7]]);
        assert_eq!(flags_offset & MF_FLAG, 0);
        assert_eq!(flags_offset & OFFSET_MASK, 0);
        // Check payload matches.
        assert_eq!(&pkt[IP_HEADER_MIN_SIZE..], payload);

        clear_fragment_cache();
    }

    #[test]
    fn test_fragment_payload_multi() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();

        // Large payload that requires fragmentation.
        // Use MTU of 576 (minimum IPv4 MTU based on RFC 791).
        let mtu = 76; // Small MTU for testing: 20 (header) + 56 (data per fragment)
        let payload_len = 100;
        let payload: Vec<u8> = (0..payload_len).map(|i| i as u8).collect();

        let src_ip: u32 = 0x0A00020F;
        let dst_ip: u32 = 0x0100A8C0;
        let id: u16 = 0x5678;

        let fragments = fragment_payload(&payload, src_ip, dst_ip, id, 6, mtu);

        // With 76 MTU: max data per frag = ((76-20)/8)*8 = 56 bytes.
        // 100 bytes -> 2 fragments: 56 + 44.
        assert_eq!(
            fragments.len(),
            2,
            "100 bytes with 56 data/frag should produce 2 fragments"
        );

        // Check fragment 0.
        let f0 = &fragments[0];
        let f0_total = u16::from_be_bytes([f0[2], f0[3]]);
        assert_eq!(f0_total, 20 + 56);
        let f0_flags = u16::from_be_bytes([f0[6], f0[7]]);
        assert!(f0_flags & MF_FLAG != 0, "first fragment should have MF set");
        assert_eq!(
            f0_flags & OFFSET_MASK,
            0,
            "first fragment offset should be 0"
        );
        assert_eq!(&f0[20..], &payload[0..56]);

        // Check fragment 1 (last).
        let f1 = &fragments[1];
        let f1_total = u16::from_be_bytes([f1[2], f1[3]]);
        assert_eq!(f1_total, 20 + 44);
        let f1_flags = u16::from_be_bytes([f1[6], f1[7]]);
        assert!(
            f1_flags & MF_FLAG == 0,
            "last fragment should NOT have MF set"
        );
        assert_eq!(
            f1_flags & OFFSET_MASK,
            7,
            "last fragment offset in units = 56/8 = 7"
        );
        assert_eq!(&f1[20..], &payload[56..]);

        clear_fragment_cache();
    }

    #[test]
    fn test_fragment_payload_exact_mtu() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();

        // Payload exactly fills one MTU-sized fragment.
        let mtu = 60;
        let max_data = ((mtu - 20) / 8) * 8; // 32 bytes for MTU=60
        let payload: Vec<u8> = vec![0xABu8; max_data];

        let fragments = fragment_payload(&payload, 0x0A00020F, 0x0100A8C0, 0x1111, 17, mtu);

        assert_eq!(
            fragments.len(),
            1,
            "payload exactly fitting MTU should produce 1 fragment"
        );
        let f0 = &fragments[0];
        // Should be a non-fragment (offset=0, MF=0).
        let flags = u16::from_be_bytes([f0[6], f0[7]]);
        assert_eq!(flags & MF_FLAG, 0);
        assert_eq!(flags & OFFSET_MASK, 0);

        clear_fragment_cache();
    }

    #[test]
    fn test_reassembly_different_ids_independent() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();

        let src_ip: u32 = 0x0A00020F;
        let dst_ip: u32 = 0x0100A8C0;

        // Two interleaved datagrams with different IDs.
        let pkt1a = make_fragment(0xAAAA, 0, true, &[1u8; 8], src_ip, dst_ip, 6);
        let pkt2a = make_fragment(0xBBBB, 0, true, &[2u8; 8], src_ip, dst_ip, 6);
        let pkt1b = make_fragment(0xAAAA, 8, false, &[3u8; 8], src_ip, dst_ip, 6);
        let pkt2b = make_fragment(0xBBBB, 8, false, &[4u8; 8], src_ip, dst_ip, 6);

        let now = 100;

        // Submit interleaved — both should complete independently.
        let r = try_reassemble(&pkt1a, now).unwrap();
        assert!(r.is_none());

        let r = try_reassemble(&pkt2a, now).unwrap();
        assert!(r.is_none());

        let r = try_reassemble(&pkt1b, now).unwrap();
        assert!(r.is_some(), "datagram A should complete");
        let mut expected_a = alloc::vec![1u8; 8];
        expected_a.extend_from_slice(&[3u8; 8]);
        assert_eq!(r.unwrap(), expected_a);

        let r = try_reassemble(&pkt2b, now).unwrap();
        assert!(r.is_some(), "datagram B should complete");
        let mut expected_b = alloc::vec![2u8; 8];
        expected_b.extend_from_slice(&[4u8; 8]);
        assert_eq!(r.unwrap(), expected_b);

        clear_fragment_cache();
    }

    #[test]
    fn test_fragment_stats() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        reset_fragment_stats();

        let stats = get_fragment_stats();
        assert_eq!(stats.fragments_received, 0);
        assert_eq!(stats.datagrams_reassembled, 0);

        // Perform a successful reassembly and check that stats updated.
        let src_ip: u32 = 0x0A00020F;
        let dst_ip: u32 = 0x0100A8C0;
        let id: u16 = 0x9999;
        let payload: Vec<u8> = vec![0x42u8; 16];

        let p1 = make_fragment(id, 0, true, &payload[..8], src_ip, dst_ip, 6);
        let p2 = make_fragment(id, 8, false, &payload[8..], src_ip, dst_ip, 6);

        let _ = try_reassemble(&p1, 100);
        let _ = try_reassemble(&p2, 100).unwrap();

        let stats = get_fragment_stats();
        assert_eq!(stats.fragments_received, 2);
        assert_eq!(stats.datagrams_reassembled, 1);

        clear_fragment_cache();
    }

    #[test]
    fn test_clear_fragment_cache() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();

        let src_ip: u32 = 0x0A00020F;
        let dst_ip: u32 = 0x0100A8C0;
        let pkt = make_fragment(0x1234, 0, true, &[0u8; 8], src_ip, dst_ip, 6);
        let _ = try_reassemble(&pkt, 100);

        assert_eq!(FRAGMENT_CACHE.lock().len(), 1, "cache should have 1 entry");

        clear_fragment_cache();
        assert!(
            FRAGMENT_CACHE.lock().is_empty(),
            "cache should be empty after clear"
        );

        clear_fragment_cache();
    }

    #[test]
    fn test_too_many_fragments() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();

        let src_ip = 0x0A00020Fu32;
        let dst_ip = 0x0100A8C0u32;
        let id = 0x7777u16;

        // Add MAX_FRAGMENTS_PER_DATAGRAM fragments (all MF=1, no last fragment yet).
        for i in 0..MAX_FRAGMENTS_PER_DATAGRAM {
            let offset = i * 8;
            let pkt = make_fragment(id, offset, true, &[0u8; 8], src_ip, dst_ip, 6);
            let r = try_reassemble(&pkt, 100);
            assert!(r.is_ok(), "fragment {} should be accepted", i);
        }

        // One more fragment should be rejected.
        let pkt_extra = make_fragment(
            id,
            MAX_FRAGMENTS_PER_DATAGRAM * 8,
            true,
            &[0u8; 8],
            src_ip,
            dst_ip,
            6,
        );
        let r = try_reassemble(&pkt_extra, 100);
        assert!(r.is_err(), "extra fragment should be rejected");
        assert_eq!(r.unwrap_err(), "fragment: too many fragments for datagram");

        clear_fragment_cache();
    }

    #[test]
    fn test_non_aligned_intermediate_fragment() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();

        // An intermediate fragment with payload not aligned to 8 bytes should
        // be rejected.
        let src_ip = 0x0A00020Fu32;
        let dst_ip = 0x0100A8C0u32;
        let id = 0x8888u16;

        let pkt = make_fragment(id, 8, true, &[0u8; 7], src_ip, dst_ip, 6);
        let r = try_reassemble(&pkt, 100);
        assert!(
            r.is_err(),
            "non-aligned intermediate fragment should be rejected"
        );

        clear_fragment_cache();
    }

    #[test]
    fn test_fragment_key_equality() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();

        let k1 = FragmentKey {
            src_ip: 0x0A00020F,
            dst_ip: 0x0100A8C0,
            identification: 0x1234,
            protocol: 6,
        };
        let k2 = FragmentKey {
            src_ip: 0x0A00020F,
            dst_ip: 0x0100A8C0,
            identification: 0x1234,
            protocol: 6,
        };
        let k3 = FragmentKey {
            src_ip: 0x0A00020F,
            dst_ip: 0x0100A8C0,
            identification: 0x5678,
            protocol: 6,
        };

        assert_eq!(k1, k2);
        assert_ne!(k1, k3);
    }

    #[test]
    fn test_fragment_payload_zero_length() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();

        // Zero-length payload should produce one fragment.
        let payload: &[u8] = &[];
        let mtu = 1500;
        let fragments = fragment_payload(payload, 0x0A00020F, 0x0100A8C0, 0x1234, 6, mtu);

        assert_eq!(
            fragments.len(),
            1,
            "empty payload should produce 1 fragment"
        );
        let pkt = &fragments[0];
        let total_len = u16::from_be_bytes([pkt[2], pkt[3]]);
        assert_eq!(total_len as usize, IP_HEADER_MIN_SIZE);

        clear_fragment_cache();
    }

    #[test]
    fn test_fragment_payload_allocation() {
        let _guard = crate::TEST_SERIAL_LOCK.lock();
        reset_fragment_stats();
        clear_fragment_cache();

        // Large payload: verify all fragments together reconstruct the original.
        let mtu = 500;
        let max_data = ((mtu - 20) / 8) * 8; // 480 bytes per fragment
        let payload_len = 1200;
        let payload: Vec<u8> = (0..payload_len).map(|i| (i % 256) as u8).collect();

        let src_ip = 0x0A00020Fu32;
        let dst_ip = 0x0100A8C0u32;
        let id = 0xABCDu16;

        let fragments = fragment_payload(&payload, src_ip, dst_ip, id, 6, mtu);

        // 1200 bytes with 480 data/frag = 3 fragments.
        assert_eq!(fragments.len(), 3);

        // Verify checksums are valid.
        // internet_checksum() over the header including the stored checksum
        // field should yield 0 for a valid checksum.
        for f in &fragments {
            let result = crate::net::tcp::internet_checksum(&f[..IP_HEADER_MIN_SIZE]);
            assert_eq!(result, 0, "IP checksum should be valid");
        }

        // Reassemble the fragments to verify they match the original.
        let mut reconstructed = vec![0u8; payload_len];
        for f in &fragments {
            let offset = (usize::from(u16::from_be_bytes([f[6], f[7]]) & OFFSET_MASK)) * 8;
            let data = &f[IP_HEADER_MIN_SIZE..];
            reconstructed[offset..offset + data.len()].copy_from_slice(data);
        }

        assert_eq!(
            payload, reconstructed,
            "reassembled payload should match original"
        );

        clear_fragment_cache();
    }
}
