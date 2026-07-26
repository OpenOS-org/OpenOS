//! ACPI table parsing for `x86_64`.
//!
//! Parses the Root System Description Pointer (RSDP), Root/Extended System
//! Description Tables (RSDT/XSDT), and the Multiple APIC Description Table
//! (MADT) to extract CPU topology, I/O APIC, and interrupt routing information.
//!
//! All ACPI table casts from `*const u8` to more-strictly-aligned types are safe
//! because ACPI tables are guaranteed to be naturally aligned by the firmware.
//!
//! ## References
//!
//! - [ACPI 6.4 Specification](https://uefi.org/specifications/ACPI/6.4)
//! - RSDP: Section 5.2.5
//! - RSDT/XSDT: Section 5.2.7/5.2.8
//! - MADT: Section 5.2.12

// ACPI tables are guaranteed to be naturally aligned by the firmware, so
// casts from `*const u8` to more-strictly-aligned types are safe.
#![allow(clippy::cast_ptr_alignment)]

use alloc::vec::Vec;

use crate::memory::phys_to_virt;
use crate::serial_println;

/// RSDP signature: "RSD PTR " (8 bytes, with trailing space).
const RSDP_SIGNATURE: &[u8; 8] = b"RSD PTR ";

/// RSDT signature: "RSDT".
const RSDT_SIGNATURE: &[u8; 4] = b"RSDT";

/// XSDT signature: "XSDT".
const XSDT_SIGNATURE: &[u8; 4] = b"XSDT";

/// MADT (Multiple APIC Description Table) signature: "APIC".
const MADT_SIGNATURE: &[u8; 4] = b"APIC";

/// Physical address range start for RSDP search (legacy EBDA area).
const RSDP_SEARCH_START: u64 = 0xE0000;

/// Physical address range end for RSDP search (end of legacy BIOS area).
const RSDP_SEARCH_END: u64 = 0xFFFFF;

/// RSDP is always aligned to 16 bytes within the search range.
const RSDP_SEARCH_STEP: u64 = 16;

/// MADT record type: Local APIC.
const MADT_TYPE_LOCAL_APIC: u8 = 0;

/// MADT record type: I/O APIC.
const MADT_TYPE_IO_APIC: u8 = 1;

/// MADT record type: Interrupt Source Override.
const MADT_TYPE_INT_SRC_OVERRIDE: u8 = 2;

/// Maximum CPUs the kernel supports.
const MAX_CPUS: usize = 256;

/// Errors that can occur during ACPI parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpiError {
    /// RSDP was not found in the legacy search range.
    RsdpNotFound,
    /// RSDP checksum validation failed.
    RsdpChecksumInvalid,
    /// RSDT/XSDT signature did not match expected value.
    InvalidTableSignature,
    /// RSDT/XSDT checksum validation failed.
    TableChecksumInvalid,
    /// MADT was not found in the RSDT/XSDT entries.
    MadtNotFound,
}

/// Interrupt source override from the MADT.
///
/// Describes how a legacy ISA interrupt (e.g., IRQ 0..15) is remapped
/// to a different GSI (Global System Interrupt) input on the I/O APIC.
/// For example, IRQ 0 (timer) is typically overridden to GSI 2 on
/// systems with an I/O APIC.
#[derive(Debug, Clone, Copy)]
pub struct IntSrcOverride {
    /// Bus source (0 = ISA).
    pub bus: u8,
    /// IRQ number on the bus (e.g., 0 for timer).
    pub irq: u8,
    /// Global System Interrupt the IRQ is mapped to.
    pub gsi: u32,
    /// Flags (polarity and trigger mode).
    pub flags: u16,
}

/// Parsed ACPI information relevant to kernel initialization.
///
/// Contains the Local APIC and I/O APIC addresses discovered from the MADT,
/// along with CPU topology (LAPIC IDs) and interrupt source overrides.
#[derive(Debug)]
pub struct AcpiInfo {
    /// Physical address of the Local APIC (from MADT header or default 0xFEE00000).
    pub local_apic_addr: u64,
    /// Physical address of the I/O APIC (from the first I/O APIC entry).
    pub io_apic_addr: u64,
    /// Number of enabled CPUs discovered.
    pub cpu_count: u32,
    /// Local APIC IDs of each discovered CPU.
    pub cpu_lapic_ids: [u8; MAX_CPUS],
    /// Interrupt source overrides from the MADT.
    pub int_src_overrides: Vec<IntSrcOverride>,
}

/// Raw RSDP (Root System Description Pointer) structure.
///
/// ACPI 1.0 compatible layout. Revision >= 2 adds XSDT address.
/// The RSDP is located by searching physical memory for the signature
/// "RSD PTR " on 16-byte boundaries in `0xE0000..0xFFFFF`.
#[repr(C, packed)]
struct Rsdp {
    /// "RSD PTR " signature.
    signature: [u8; 8],
    /// Checksum of first 20 bytes (ACPI 1.0).
    checksum: u8,
    /// OEM identifier (6 characters).
    oem_id: [u8; 6],
    /// Revision: 0 = ACPI 1.0 (RSDT only), 2 = ACPI 2.0+ (XSDT available).
    revision: u8,
    /// Physical address of RSDT (ACPI 1.0).
    rsdt_addr: u32,
}

/// XSDT extension fields present in RSDP revision >= 2.
#[repr(C, packed)]
struct RsdpExtended {
    /// All fields from the base RSDP.
    base: Rsdp,
    /// Length of the entire RSDP structure (including extension).
    length: u32,
    /// Physical address of XSDT (64-bit).
    xsdt_addr: u64,
    /// Extended checksum (covers entire structure).
    extended_checksum: u8,
}

/// Generic ACPI System Description Table header.
///
/// Every SDT (RSDT, XSDT, MADT, FADT, etc.) begins with this 36-byte header.
/// The `signature` field identifies the table type.
#[repr(C, packed)]
struct SdtHeader {
    /// 4-character table signature (e.g., "RSDT", "APIC", "FACP").
    signature: [u8; 4],
    /// Length of the entire table including this header.
    length: u32,
    /// Revision of the table structure.
    revision: u8,
    /// Checksum of the entire table (sum of all bytes must be 0).
    checksum: u8,
    /// OEM identifier (6 characters).
    oem_id: [u8; 6],
    /// OEM table identifier (8 characters).
    oem_table_id: [u8; 8],
    /// OEM revision number.
    oem_revision: u32,
    /// Creator ID (4 characters, typically the ASL compiler).
    creator_id: u32,
    /// Creator revision number.
    creator_revision: u32,
}

/// MADT (Multiple APIC Description Table) header.
///
/// Follows the SDT header in the MADT. Contains the Local APIC address
/// and flags, followed by variable-length APIC structure records.
#[repr(C, packed)]
struct MadtHeader {
    /// SDT header (signature is "APIC").
    sdt: SdtHeader,
    /// Physical address of Local APIC (typically 0xFEE00000).
    local_apic_addr: u32,
    /// Flags: bit 0 = `PCAT_COMPAT` (dual 8259 PICs present).
    flags: u32,
}

/// MADT Local APIC entry (type 0).
///
/// Describes one processor's Local APIC. Each enabled CPU has one entry.
#[repr(C, packed)]
struct MadtLocalApicEntry {
    /// Record type (0 = Local APIC).
    entry_type: u8,
    /// Length of this record (8 bytes).
    length: u8,
    /// ACPI processor ID.
    processor_id: u8,
    /// Local APIC ID.
    apic_id: u8,
    /// Flags: bit 0 = enabled, bit 1 = online capable.
    flags: u32,
}

/// MADT I/O APIC entry (type 1).
///
/// Describes one I/O APIC in the system.
#[repr(C, packed)]
struct MadtIoApicEntry {
    /// Record type (1 = I/O APIC).
    entry_type: u8,
    /// Length of this record (12 bytes).
    length: u8,
    /// I/O APIC ID.
    io_apic_id: u8,
    /// Reserved (must be 0).
    _reserved: u8,
    /// Physical address of this I/O APIC's registers.
    io_apic_addr: u32,
    /// Global System Interrupt base for this I/O APIC.
    gsi_base: u32,
}

/// MADT Interrupt Source Override entry (type 2).
///
/// Describes how a bus-relative interrupt (e.g., ISA IRQ) is mapped
/// to a Global System Interrupt on the I/O APIC.
#[repr(C, packed)]
struct MadtIntSrcOverrideEntry {
    /// Record type (2 = Interrupt Source Override).
    entry_type: u8,
    /// Length of this record (10 bytes).
    length: u8,
    /// Bus (0 = ISA).
    bus: u8,
    /// IRQ on the bus.
    irq: u8,
    /// Global System Interrupt.
    gsi: u32,
    /// Flags (polarity and trigger mode).
    flags: u16,
}

/// Validate an ACPI table checksum.
///
/// Sums all bytes in the table; the result must be zero for a valid table.
/// This catches memory corruption and firmware bugs.
fn validate_checksum(data: *const u8, length: usize) -> bool {
    // SAFETY: `data` points to ACPI table memory mapped via phys_to_virt.
    // The bootloader guarantees all physical memory is mapped, so reads are
    // valid for `length` bytes. We only read, never write.
    let bytes = unsafe { core::slice::from_raw_parts(data, length) };
    let sum: u8 = bytes.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
    sum == 0
}

/// Search for the RSDP in the legacy BIOS area (0xE0000..0xFFFFF).
///
/// The RSDP signature "RSD PTR " appears on 16-byte boundaries. We scan
/// the entire range and return the first valid match.
fn find_rsdp() -> Result<*const Rsdp, AcpiError> {
    let mut addr = RSDP_SEARCH_START;
    while addr < RSDP_SEARCH_END {
        let virt = phys_to_virt(addr) as *const u8;

        // SAFETY: `addr` is within the legacy BIOS area (0xE0000..0xFFFFF),
        // which is always mapped by the bootloader's physical memory mapping.
        // We read 8 bytes for the signature check.
        let sig = unsafe { core::slice::from_raw_parts(virt, RSDP_SIGNATURE.len()) };
        if sig == RSDP_SIGNATURE.as_slice() {
            let rsdp = virt.cast::<Rsdp>();

            // Validate the ACPI 1.0 checksum (first 20 bytes).
            if validate_checksum(virt, core::mem::size_of::<Rsdp>()) {
                return Ok(rsdp);
            }
            // Found signature but checksum failed — keep searching.
        }
        addr += RSDP_SEARCH_STEP;
    }
    Err(AcpiError::RsdpNotFound)
}

/// Parse the RSDT and return the list of SDT physical addresses.
///
/// The RSDT contains an array of 32-bit physical addresses pointing to
/// other System Description Tables (MADT, FADT, etc.).
fn parse_rsdt(rsdt_phys: u32) -> Result<Vec<u64>, AcpiError> {
    let virt = phys_to_virt(u64::from(rsdt_phys)) as *const u8;

    // SAFETY: The RSDT physical address comes from the validated RSDP.
    // We read the SDT header to get the table length for checksum validation.
    let header = unsafe { &*virt.cast::<SdtHeader>() };

    if header.signature != *RSDT_SIGNATURE {
        return Err(AcpiError::InvalidTableSignature);
    }

    let table_len = header.length as usize;
    if table_len < core::mem::size_of::<SdtHeader>() {
        return Err(AcpiError::InvalidTableSignature);
    }

    if !validate_checksum(virt, table_len) {
        return Err(AcpiError::TableChecksumInvalid);
    }

    // Entry array starts after the 36-byte SDT header.
    // SAFETY: We validated the table length and checksum, so all entries
    // within `table_len` bytes are valid.
    let entries_start = unsafe { virt.add(core::mem::size_of::<SdtHeader>()) }.cast::<u32>();
    let num_entries = (table_len - core::mem::size_of::<SdtHeader>()) / 4;
    let mut addrs = Vec::with_capacity(num_entries);

    for i in 0..num_entries {
        // SAFETY: `i` is within bounds — we computed `num_entries` from the
        // validated table length.
        let entry = unsafe { *entries_start.add(i) };
        addrs.push(u64::from(entry));
    }

    Ok(addrs)
}

/// Parse the XSDT and return the list of SDT physical addresses.
///
/// The XSDT is the 64-bit version of the RSDT, used when the RSDP
/// revision is >= 2 (ACPI 2.0+). Each entry is a 64-bit physical address.
fn parse_xsdt(xsdt_phys: u64) -> Result<Vec<u64>, AcpiError> {
    let virt = phys_to_virt(xsdt_phys) as *const u8;

    // SAFETY: The XSDT physical address comes from the validated RSDP.
    let header = unsafe { &*virt.cast::<SdtHeader>() };

    if header.signature != *XSDT_SIGNATURE {
        return Err(AcpiError::InvalidTableSignature);
    }

    let table_len = header.length as usize;
    if table_len < core::mem::size_of::<SdtHeader>() {
        return Err(AcpiError::InvalidTableSignature);
    }

    if !validate_checksum(virt, table_len) {
        return Err(AcpiError::TableChecksumInvalid);
    }

    // SAFETY: We validated the table length and checksum, so the pointer
    // arithmetic and subsequent reads are within bounds.
    let entries_start = unsafe { virt.add(core::mem::size_of::<SdtHeader>()) }.cast::<u64>();
    let num_entries = (table_len - core::mem::size_of::<SdtHeader>()) / 8;
    let mut addrs = Vec::with_capacity(num_entries);

    for i in 0..num_entries {
        // SAFETY: `i` is within bounds — we computed `num_entries` from the
        // validated table length.
        let entry = unsafe { *entries_start.add(i) };
        addrs.push(entry);
    }

    Ok(addrs)
}

/// Parse the MADT (Multiple APIC Description Table).
///
/// Iterates the variable-length APIC structure records after the MADT header
/// to discover Local APICs (CPUs), I/O APICs, and interrupt source overrides.
fn parse_madt(madt_phys: u64, info: &mut AcpiInfo) -> Result<(), AcpiError> {
    let virt = phys_to_virt(madt_phys) as *const u8;

    // SAFETY: `madt_phys` is a valid SDT address from the RSDT/XSDT.
    let madt = unsafe { &*virt.cast::<MadtHeader>() };

    if madt.sdt.signature != *MADT_SIGNATURE {
        return Err(AcpiError::InvalidTableSignature);
    }

    let table_len = madt.sdt.length as usize;
    if table_len < core::mem::size_of::<MadtHeader>() {
        return Err(AcpiError::InvalidTableSignature);
    }

    if !validate_checksum(virt, table_len) {
        return Err(AcpiError::TableChecksumInvalid);
    }

    // Use the Local APIC address from the MADT header.
    info.local_apic_addr = u64::from(madt.local_apic_addr);

    // Walk variable-length records after the fixed MADT header.
    let mut offset = core::mem::size_of::<MadtHeader>();
    while offset + 2 <= table_len {
        // SAFETY: We stay within `table_len` bytes, which we validated via checksum.
        let entry_type = unsafe { *virt.add(offset) };
        let entry_len = unsafe { *virt.add(offset + 1) } as usize;

        if entry_len < 2 {
            // Minimum entry length is 2 (type + length fields).
            break;
        }

        if offset + entry_len > table_len {
            break;
        }

        match entry_type {
            MADT_TYPE_LOCAL_APIC => {
                if entry_len >= core::mem::size_of::<MadtLocalApicEntry>() {
                    // SAFETY: We checked offset + entry_len <= table_len.
                    let entry = unsafe { &*virt.add(offset).cast::<MadtLocalApicEntry>() };
                    // Flags bit 0: 1 = enabled.
                    if entry.flags & 1 != 0 && (info.cpu_count as usize) < MAX_CPUS {
                        let idx = info.cpu_count as usize;
                        info.cpu_lapic_ids[idx] = entry.apic_id;
                        info.cpu_count += 1;
                    }
                }
            }
            MADT_TYPE_IO_APIC => {
                if entry_len >= core::mem::size_of::<MadtIoApicEntry>() {
                    // SAFETY: We checked offset + entry_len <= table_len.
                    let entry = unsafe { &*virt.add(offset).cast::<MadtIoApicEntry>() };
                    // Use the first I/O APIC discovered.
                    if info.io_apic_addr == 0 {
                        info.io_apic_addr = u64::from(entry.io_apic_addr);
                    }
                }
            }
            MADT_TYPE_INT_SRC_OVERRIDE
                if entry_len >= core::mem::size_of::<MadtIntSrcOverrideEntry>() =>
            {
                // SAFETY: We checked offset + entry_len <= table_len.
                let entry = unsafe { &*virt.add(offset).cast::<MadtIntSrcOverrideEntry>() };
                info.int_src_overrides.push(IntSrcOverride {
                    bus: entry.bus,
                    irq: entry.irq,
                    gsi: entry.gsi,
                    flags: entry.flags,
                });
            }
            _ => {
                // Unknown MADT record type — skip.
            }
        }

        offset += entry_len;
    }

    Ok(())
}

/// Parse ACPI tables and return system topology information.
///
/// This is the main entry point. It:
/// 1. Searches for the RSDP in the legacy BIOS area (0xE0000..0xFFFFF)
/// 2. Validates the RSDP checksum
/// 3. Parses the RSDT (or XSDT for ACPI 2.0+) to find all SDTs
/// 4. Locates and parses the MADT for CPU/APIC information
///
/// # Errors
///
/// Returns `AcpiError` if the RSDP is not found, any checksum fails,
/// or the MADT is missing.
///
/// # Panics
///
/// Panics if `phys_to_virt()` is called before `set_physical_memory_offset()`.
pub fn parse(rsdp_addr: Option<u64>) -> Result<AcpiInfo, AcpiError> {
    let rsdp = if let Some(addr) = rsdp_addr {
        // UEFI: RSDP address provided by BootInfo.
        serial_println!("[ACPI] Using RSDP from BootInfo at {:#x}", addr);
        let virt = phys_to_virt(addr) as *const Rsdp;
        if !validate_checksum(virt.cast::<u8>(), core::mem::size_of::<Rsdp>()) {
            return Err(AcpiError::RsdpChecksumInvalid);
        }
        virt
    } else {
        find_rsdp()?
    };

    // SAFETY: `rsdp` was found by scanning valid memory and passed checksum.
    let rsdp_ref = unsafe { &*rsdp };

    // Copy packed fields to local variables to avoid unaligned references
    // in format_args (format_args creates references, which must be aligned).
    let revision = rsdp_ref.revision;
    let rsdt_addr = rsdp_ref.rsdt_addr;

    serial_println!(
        "[ACPI] RSDP found at phys {:#x}, revision {}",
        rsdp as u64,
        revision
    );

    // Collect SDT addresses from RSDT or XSDT.
    let sdt_addrs = if revision >= 2 {
        // SAFETY: We checked revision >= 2, so the extended fields are present.
        let extended = unsafe { &*rsdp.cast::<RsdpExtended>() };
        let xsdt_phys = extended.xsdt_addr;
        if xsdt_phys != 0 {
            serial_println!("[ACPI] Using XSDT at phys {:#x}", xsdt_phys);
            parse_xsdt(xsdt_phys)?
        } else {
            // Fallback to RSDT if XSDT address is zero (firmware bug).
            serial_println!("[ACPI] XSDT address is zero, falling back to RSDT");
            parse_rsdt(rsdt_addr)?
        }
    } else {
        serial_println!("[ACPI] Using RSDT at phys {:#x}", rsdt_addr);
        parse_rsdt(rsdt_addr)?
    };

    // Default Local APIC address (standard x86).
    const DEFAULT_LAPIC_ADDR: u64 = 0xFEE0_0000;

    let mut info = AcpiInfo {
        local_apic_addr: DEFAULT_LAPIC_ADDR,
        io_apic_addr: 0,
        cpu_count: 0,
        cpu_lapic_ids: [0; MAX_CPUS],
        int_src_overrides: Vec::new(),
    };

    // Search for the MADT among the SDT entries.
    let mut found_madt = false;
    for &addr in &sdt_addrs {
        let virt = phys_to_virt(addr) as *const u8;

        // SAFETY: `addr` is a valid SDT address from the RSDT/XSDT.
        // We read 4 bytes for the signature check.
        let sig = unsafe { core::slice::from_raw_parts(virt, 4) };
        if sig == MADT_SIGNATURE.as_slice() {
            parse_madt(addr, &mut info)?;
            found_madt = true;
            break;
        }
    }

    if !found_madt {
        return Err(AcpiError::MadtNotFound);
    }

    serial_println!(
        "[ACPI] MADT: LAPIC={:#x}, IOAPIC={:#x}, CPUs={}, overrides={}",
        info.local_apic_addr,
        info.io_apic_addr,
        info.cpu_count,
        info.int_src_overrides.len()
    );

    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rsdp_signature_constant() {
        assert_eq!(RSDP_SIGNATURE, b"RSD PTR ");
    }

    #[test]
    fn test_validate_checksum_valid() {
        // A table where all bytes sum to 0 (mod 256).
        let data: [u8; 4] = [10, 20, 30, 196];
        assert!(validate_checksum(data.as_ptr(), 4));
    }

    #[test]
    fn test_validate_checksum_invalid() {
        let data: [u8; 4] = [10, 20, 30, 30];
        assert!(!validate_checksum(data.as_ptr(), 4));
    }

    #[test]
    fn test_acpi_error_debug() {
        let err = AcpiError::RsdpNotFound;
        assert_eq!(format!("{err:?}"), "RsdpNotFound");
    }

    #[test]
    fn test_int_src_override_fields() {
        let iso = IntSrcOverride {
            bus: 0,
            irq: 0,
            gsi: 2,
            flags: 0x0D,
        };
        assert_eq!(iso.bus, 0);
        assert_eq!(iso.irq, 0);
        assert_eq!(iso.gsi, 2);
        assert_eq!(iso.flags, 0x0D);
    }

    #[test]
    fn test_acpi_info_defaults() {
        let info = AcpiInfo {
            local_apic_addr: 0xFEE0_0000,
            io_apic_addr: 0,
            cpu_count: 0,
            cpu_lapic_ids: [0; MAX_CPUS],
            int_src_overrides: Vec::new(),
        };
        assert_eq!(info.local_apic_addr, 0xFEE0_0000);
        assert_eq!(info.io_apic_addr, 0);
        assert_eq!(info.cpu_count, 0);
    }

    #[test]
    fn test_madt_signature_constant() {
        assert_eq!(MADT_SIGNATURE, b"APIC");
    }
}
