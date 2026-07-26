//! Minimal ELF64 parser and loader.
//!
//! Parses an ELF64 executable, loads `PT_LOAD` segments into physical memory,
//! and sets up page table entries with correct permissions. Designed for
//! `#![no_std]` — no heap allocation, zero external dependencies.
//!
//! ## ELF64 Format Reference
//!
//! - Header: 64 bytes at offset 0
//! - Program headers: 56 bytes each, starting at `e_phoff`
//! - Only `PT_LOAD` segments (type 1) are loaded into memory
//! - `p_flags` determines page permissions: `PF_X` = executable, `PF_W` = writable

/// ELF magic bytes: `\x7fELF`.
const ELFMAG: [u8; 4] = [0x7f, b'E', b'L', b'F'];
/// ELFCLASS64: 64-bit format.
const ELFCLASS64: u8 = 2;
/// ELFDATA2LSB: little-endian.
const ELFDATA2LSB: u8 = 1;
/// `EM_X86_64`: x86-64 architecture.
const EM_X86_64: u16 = 62;
/// `ET_EXEC`: executable file.
const ET_EXEC: u16 = 2;
/// `ET_DYN`: position-independent executable (PIE).
const ET_DYN: u16 = 3;
/// `PT_LOAD`: loadable segment.
const PT_LOAD: u32 = 1;
/// `PT_DYNAMIC`: dynamic linking information.
const PT_DYNAMIC: u32 = 2;
/// `PT_INTERP`: interpreter path (e.g., `/lib/ld.so`).
const PT_INTERP: u32 = 3;
/// `PF_X`: executable segment.
const PF_X: u32 = 0x1;
/// `PF_W`: writable segment.
const PF_W: u32 = 0x2;

// ─── Dynamic section tags ───

/// End of dynamic section.
const DT_NULL: i64 = 0;
/// Name of needed library.
const DT_NEEDED: i64 = 1;
/// String table address.
const DT_STRTAB: i64 = 5;
/// String table size.
const DT_STRSZ: i64 = 10;
/// Symbol table address.
const DT_SYMTAB: i64 = 6;
/// Size of symbol table entry.
const DT_SYMENT: i64 = 11;
/// PLT relocation table.
const DT_JMPREL: i64 = 23;
/// Size of PLT relocation table.
const DT_PLTRELSZ: i64 = 2;
/// PLT relocation type.
const DT_PLTREL: i64 = 20;
/// RELA relocation table.
const DT_RELA: i64 = 7;
/// Size of RELA table.
const DT_RELASZ: i64 = 8;
/// Size of RELA entry.
const DT_RELAENT: i64 = 9;
/// Global Offset Table address.
const DT_PLTGOT: i64 = 3;

// ─── Relocation types (x86_64) ───

/// No relocation.
const R_X86_64_NONE: u32 = 0;
/// Direct 64-bit relocation.
const R_X86_64_64: u32 = 1;
/// GOT entry (G + GOT + A).
const R_X86_64_GLOB_DAT: u32 = 6;
/// Jump slot (lazy binding).
const R_X86_64_JUMP_SLOT: u32 = 7;
/// Relative relocation (B + A).
const R_X86_64_RELATIVE: u32 = 8;

/// Errors that can occur during ELF parsing or loading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfError {
    /// Not an ELF file (bad magic).
    BadMagic,
    /// Not a 64-bit ELF.
    NotElf64,
    /// Not little-endian.
    NotLittleEndian,
    /// Not an x86-64 binary.
    WrongArchitecture,
    /// Not an executable or PIE.
    NotExecutable,
    /// A `PT_LOAD` segment has invalid addresses.
    InvalidSegment,
    /// Two `PT_LOAD` segments overlap in virtual address space.
    SegmentOverlap,
    /// Out of physical frames.
    OutOfMemory,
}

/// Parsed ELF64 header fields (only what we need for loading).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Elf64Header {
    /// Virtual address of the entry point.
    pub entry: u64,
    /// File offset of the program header table.
    pub phoff: u64,
    /// Number of program header entries.
    pub phnum: u16,
    /// ELF object type (ET_EXEC=2, ET_DYN=3).
    pub e_type: u16,
}

/// A single program header (`PT_LOAD` segment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_field_names)]
pub struct ProgramHeader {
    /// Segment type (should be `PT_LOAD` for loadable segments).
    pub p_type: u32,
    /// Segment flags (`PF_X`, `PF_W`, `PF_R`).
    pub p_flags: u32,
    /// Offset of segment data in the ELF file.
    pub p_offset: u64,
    /// Virtual address to load the segment at.
    pub p_vaddr: u64,
    /// Size of segment data in the file.
    pub p_filesz: u64,
    /// Size of segment in memory (includes .bss zero-fill).
    pub p_memsz: u64,
}

/// Result of a successful ELF load: the entry point and stack pointer.
pub struct ElfLoadResult {
    /// Virtual address of the program's entry point.
    pub entry_point: u64,
    /// Virtual address of the top of the user stack (stack grows downward).
    pub stack_top: u64,
    /// Dynamic section info (if present).
    pub dynamic: Option<DynamicInfo>,
    /// ELF type (ET_EXEC=2, ET_DYN=3).
    pub e_type: u16,
    /// Virtual address of the guard page (below the stack).
    pub guard_page_virt: u64,
}

/// Information extracted from the `PT_DYNAMIC` segment.
#[derive(Debug, Clone)]
pub struct DynamicInfo {
    /// Address of the dynamic string table (`DT_STRTAB`).
    pub strtab_addr: u64,
    /// Size of the string table (`DT_STRSZ`).
    pub strtab_size: u64,
    /// Address of the symbol table (`DT_SYMTAB`).
    pub symtab_addr: u64,
    /// Size of a symbol table entry (`DT_SYMENT`, typically 24).
    pub sym_entry_size: u64,
    /// Addresses of needed libraries (from `DT_NEEDED`, indices into strtab).
    pub needed: alloc::vec::Vec<u64>,
    /// Address of PLT relocations (`DT_JMPREL`).
    pub jmprel_addr: u64,
    /// Size of PLT relocation table (`DT_PLTRELSZ`).
    pub jmprel_size: u64,
    /// Address of RELA relocations (`DT_RELA`).
    pub rela_addr: u64,
    /// Size of RELA table (`DT_RELASZ`).
    pub rela_size: u64,
    /// Address of the GOT/PLT (`DT_PLTGOT`).
    pub pltgot_addr: u64,
}

/// An ELF64 symbol table entry (from `DT_SYMTAB`).
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Elf64Sym {
    /// Symbol name (index into string table).
    pub st_name: u32,
    /// Symbol type and binding.
    pub st_info: u8,
    /// Symbol visibility.
    pub st_other: u8,
    /// Section index.
    pub st_shndx: u16,
    /// Symbol value (address).
    pub st_value: u64,
    /// Symbol size.
    pub st_size: u64,
}

/// An ELF64 RELA relocation entry.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Elf64Rela {
    /// Address of the relocation (virtual address).
    pub r_offset: u64,
    /// Relocation type and symbol index.
    pub r_info: u64,
    /// Addend.
    pub r_addend: i64,
}

impl Elf64Rela {
    /// Extract the relocation type from `r_info`.
    #[must_use]
    pub fn rel_type(&self) -> u32 {
        (self.r_info & 0xFFFF_FFFF) as u32
    }

    /// Extract the symbol index from `r_info`.
    #[must_use]
    pub fn sym_index(&self) -> u32 {
        (self.r_info >> 32) as u32
    }
}

/// Parse the ELF64 header from a byte slice.
///
/// Returns the parsed header fields if the ELF is valid and loadable.
///
/// # Errors
///
/// Returns `ElfError::BadMagic` if the magic bytes are invalid.
/// Returns `ElfError::NotElf64` if not 64-bit.
/// Returns `ElfError::NotLittleEndian` if not little-endian.
/// Returns `ElfError::WrongArchitecture` if not x86-64.
/// Returns `ElfError::NotExecutable` if not executable or PIE.
///
/// # Panics
///
/// Panics if internal byte slice conversions fail (should not happen with valid ELF data).
pub fn parse_header(data: &[u8]) -> Result<Elf64Header, ElfError> {
    if data.len() < 64 {
        return Err(ElfError::BadMagic);
    }

    // Validate magic.
    if data[0..4] != ELFMAG {
        return Err(ElfError::BadMagic);
    }
    // Must be 64-bit.
    if data[4] != ELFCLASS64 {
        return Err(ElfError::NotElf64);
    }
    // Must be little-endian.
    if data[5] != ELFDATA2LSB {
        return Err(ElfError::NotLittleEndian);
    }

    let e_type = u16::from_le_bytes([data[16], data[17]]);
    let e_machine = u16::from_le_bytes([data[18], data[19]]);
    let e_entry = u64::from_le_bytes(data[24..32].try_into().unwrap());
    let e_phoff = u64::from_le_bytes(data[32..40].try_into().unwrap());
    let e_phnum = u16::from_le_bytes(data[56..58].try_into().unwrap());

    if e_machine != EM_X86_64 {
        return Err(ElfError::WrongArchitecture);
    }
    if e_type != ET_EXEC && e_type != ET_DYN {
        return Err(ElfError::NotExecutable);
    }

    Ok(Elf64Header {
        entry: e_entry,
        phoff: e_phoff,
        phnum: e_phnum,
        e_type,
    })
}

/// Parse a single program header at the given index.
///
/// Program headers are 56 bytes each in ELF64.
///
/// # Errors
///
/// Returns `ElfError::InvalidSegment` if the program header extends beyond the data.
///
/// # Panics
///
/// Panics if internal byte slice conversions fail (should not happen with valid ELF data).
pub fn parse_program_header(
    data: &[u8],
    phoff: u64,
    index: u16,
) -> Result<ProgramHeader, ElfError> {
    let offset = phoff as usize + (index as usize) * 56;
    if offset + 56 > data.len() {
        return Err(ElfError::InvalidSegment);
    }

    let ph = &data[offset..offset + 56];
    let p_type = u32::from_le_bytes(ph[0..4].try_into().unwrap());
    let p_flags = u32::from_le_bytes(ph[4..8].try_into().unwrap());
    let p_offset = u64::from_le_bytes(ph[8..16].try_into().unwrap());
    let p_vaddr = u64::from_le_bytes(ph[16..24].try_into().unwrap());
    let p_filesz = u64::from_le_bytes(ph[32..40].try_into().unwrap());
    let p_memsz = u64::from_le_bytes(ph[40..48].try_into().unwrap());

    Ok(ProgramHeader {
        p_type,
        p_flags,
        p_offset,
        p_vaddr,
        p_filesz,
        p_memsz,
    })
}

/// Load an ELF64 executable into memory.
///
/// For each `PT_LOAD` segment:
///   1. Allocate physical frames via the frame allocator
///   2. Copy segment data from the ELF file
///   3. Zero-fill BSS (where `p_memsz > p_filesz`)
///   4. Map pages with correct permissions (RX for code, RW for data)
///
/// Also allocates a user stack at a fixed virtual address region.
///
/// # Arguments
/// - `data`: the raw ELF file bytes
/// - `map_page`: function to map a virtual page to a physical frame
///
/// # Returns
/// The entry point address and stack top address.
///
/// # Errors
///
/// Returns `ElfError` if parsing fails, segments overlap, or memory allocation fails.
pub fn load_elf<F>(data: &[u8], mut map_page: F) -> Result<ElfLoadResult, ElfError>
where
    F: FnMut(u64, u64, bool, bool), // (virt, phys, writable, executable)
{
    let header = parse_header(data)?;

    // Collect PT_LOAD segment ranges and validate no overlaps.
    let mut seg_ranges: alloc::vec::Vec<(u64, u64)> = alloc::vec::Vec::new();
    for i in 0..header.phnum {
        let ph = parse_program_header(data, header.phoff, i)?;
        if ph.p_type != PT_LOAD {
            continue;
        }
        let start_page = ph.p_vaddr & !0xFFF;
        let end_page = match ph
            .p_vaddr
            .checked_add(ph.p_memsz)
            .and_then(|v| v.checked_add(0xFFF))
        {
            Some(v) => v & !0xFFF,
            None => return Err(ElfError::InvalidSegment),
        };
        if end_page <= start_page {
            continue;
        }
        // Check against all previously collected segments for overlap.
        for &(prev_start, prev_end) in &seg_ranges {
            if start_page < prev_end && end_page > prev_start {
                return Err(ElfError::SegmentOverlap);
            }
        }
        seg_ranges.push((start_page, end_page));
    }

    // Walk program headers, load PT_LOAD segments.
    for i in 0..header.phnum {
        let ph = parse_program_header(data, header.phoff, i)?;
        if ph.p_type != PT_LOAD {
            continue;
        }

        let writable = (ph.p_flags & PF_W) != 0;
        let executable = (ph.p_flags & PF_X) != 0;

        // Calculate page-aligned range with overflow checks.
        let start_page = ph.p_vaddr & !0xFFF;
        let end_page = match ph
            .p_vaddr
            .checked_add(ph.p_memsz)
            .and_then(|v| v.checked_add(0xFFF))
        {
            Some(v) => v & !0xFFF,
            None => return Err(ElfError::InvalidSegment),
        };

        if end_page <= start_page {
            continue; // zero-size segment
        }

        let num_pages = (end_page - start_page) / 0x1000;

        // Allocate and map each page.
        for page_idx in 0..num_pages {
            let Some(virt) = start_page.checked_add(page_idx * 0x1000) else {
                return Err(ElfError::InvalidSegment);
            };
            let phys = crate::frame_alloc::alloc_frame().ok_or(ElfError::OutOfMemory)?;

            // Zero the page first.
            // SAFETY: `phys` was just allocated by `alloc_frame()`, so it's a valid,
            // exclusively-owned physical frame. `phys_to_virt` converts it to a
            // writable virtual address via the bootloader's physical memory mapping.
            let dest = crate::memory::phys_to_virt(phys) as *mut u8;
            unsafe {
                core::ptr::write_bytes(dest, 0, 4096);
            }

            // Calculate how much of this page overlaps with the segment's file data.
            let page_offset_in_seg = virt.saturating_sub(ph.p_vaddr);
            if page_offset_in_seg < ph.p_filesz {
                // This page contains some file data.
                let copy_from_seg = page_offset_in_seg;
                let copy_to_seg = page_offset_in_seg.saturating_add(0x1000).min(ph.p_filesz);
                let copy_len = copy_to_seg - copy_from_seg;

                let file_start = match ph.p_offset.checked_add(copy_from_seg) {
                    Some(v) => v as usize,
                    None => continue,
                };
                let Some(file_end) = file_start.checked_add(copy_len as usize) else {
                    continue;
                };

                if file_end <= data.len() {
                    let src = &data[file_start..file_end];
                    // SAFETY: `dest` points to a freshly-allocated frame (zeroed above).
                    // `src` is a valid slice from the ELF data. `copy_len` is bounded
                    // by the page size and validated against `data.len()`.
                    unsafe {
                        core::ptr::copy_nonoverlapping(src.as_ptr(), dest, copy_len as usize);
                    }
                }
            }
            // The rest of the page is already zeroed (covers .bss).

            // Map the page.
            map_page(virt, phys, writable, executable);
        }
    }

    // Allocate user stack: 2 pages (8 KiB) at a known virtual address.
    // We use a region above the typical ELF load address to avoid conflicts.
    let stack_virt_base = 0x0000_7FFF_FFFF_E000; // Just below 128 TiB user space limit
    let stack_pages = 2;
    let stack_top = stack_virt_base + stack_pages * 0x1000;

    for i in 0..stack_pages {
        let virt = stack_virt_base + i * 0x1000;
        let phys = crate::frame_alloc::alloc_frame().ok_or(ElfError::OutOfMemory)?;
        // SAFETY: `phys` was just allocated by `alloc_frame()`, exclusively owned.
        // `phys_to_virt` converts to a writable virtual address.
        let dest = crate::memory::phys_to_virt(phys) as *mut u8;
        unsafe {
            core::ptr::write_bytes(dest, 0, 4096);
        }
        map_page(virt, phys, true, false); // RW, not executable
    }

    // Parse PT_DYNAMIC segment if present.
    let dynamic = parse_dynamic_segment(data, header.phoff, header.phnum);

    Ok(ElfLoadResult {
        entry_point: header.entry,
        stack_top,
        dynamic,
        e_type: header.e_type,
        guard_page_virt: stack_top.saturating_sub(0x1000),
    })
}

/// Parse the `PT_DYNAMIC` segment from an ELF file.
///
/// Returns `Some(DynamicInfo)` if a valid `PT_DYNAMIC` segment is found,
/// `None` if no dynamic segment exists (static binary).
fn parse_dynamic_segment(data: &[u8], phoff: u64, phnum: u16) -> Option<DynamicInfo> {
    // Find the PT_DYNAMIC program header.
    let mut dyn_offset: u64 = 0;
    let mut dyn_filesz: u64 = 0;
    let mut found = false;

    for i in 0..phnum {
        let ph = parse_program_header(data, phoff, i).ok()?;
        if ph.p_type == PT_DYNAMIC {
            dyn_offset = ph.p_offset;
            dyn_filesz = ph.p_filesz;
            found = true;
            break;
        }
    }

    if !found {
        return None;
    }

    // Parse dynamic entries (each is 16 bytes: 8-byte tag + 8-byte value).
    let mut strtab_addr: u64 = 0;
    let mut strtab_size: u64 = 0;
    let mut symtab_addr: u64 = 0;
    let mut sym_entry_size: u64 = 24; // Default for ELF64.
    let mut needed = alloc::vec::Vec::new();
    let mut jmprel_addr: u64 = 0;
    let mut jmprel_size: u64 = 0;
    let mut rela_addr: u64 = 0;
    let mut rela_size: u64 = 0;
    let mut pltgot_addr: u64 = 0;

    let num_entries = dyn_filesz / 16;
    for i in 0..num_entries {
        let off = (dyn_offset + i * 16) as usize;
        if off + 16 > data.len() {
            break;
        }
        let tag = i64::from_le_bytes(data[off..off + 8].try_into().ok()?);
        let val = u64::from_le_bytes(data[off + 8..off + 16].try_into().ok()?);

        match tag {
            DT_NULL => break,
            DT_NEEDED => needed.push(val),
            DT_STRTAB => strtab_addr = val,
            DT_STRSZ => strtab_size = val,
            DT_SYMTAB => symtab_addr = val,
            DT_SYMENT => sym_entry_size = val,
            DT_JMPREL => jmprel_addr = val,
            DT_PLTRELSZ => jmprel_size = val,
            DT_RELA => rela_addr = val,
            DT_RELASZ => rela_size = val,
            DT_PLTGOT => pltgot_addr = val,
            _ => {}
        }
    }

    // strtab_addr is a virtual address. For the kernel-side loader, we need
    // to convert it to a file offset. But for user-space ld.so, it will have
    // the actual virtual address after loading. We store the virtual address.
    Some(DynamicInfo {
        strtab_addr,
        strtab_size,
        symtab_addr,
        sym_entry_size,
        needed,
        jmprel_addr,
        jmprel_size,
        rela_addr,
        rela_size,
        pltgot_addr,
    })
}

/// Get the name of a needed library from the dynamic string table.
///
/// # Arguments
/// - `data`: the ELF file bytes
/// - `strtab_vaddr`: virtual address of the string table
/// - `name_offset`: offset into the string table (from `DT_NEEDED`)
/// - `base_addr`: the virtual address where the ELF was loaded (for vaddr → file offset)
///
/// Returns the library name as a byte slice (null-terminated).
#[must_use]
pub fn get_needed_name(
    data: &[u8],
    strtab_vaddr: u64,
    name_offset: u64,
    base_addr: u64,
) -> Option<&[u8]> {
    // The strtab_vaddr is a virtual address. We need to find the corresponding
    // file offset. For PIE binaries, the load base is typically 0, so
    // file_offset ≈ vaddr. For ET_EXEC, we need to subtract the load base.
    let file_off = (strtab_vaddr + name_offset).checked_sub(base_addr)? as usize;
    if file_off >= data.len() {
        return None;
    }
    // Find the null terminator.
    let end = data[file_off..].iter().position(|&b| b == 0)?;
    Some(&data[file_off..file_off + end])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal ELF64 header for testing.
    fn build_elf_header(entry: u64, phoff: u64, phnum: u16, etype: u16) -> Vec<u8> {
        let mut hdr = vec![0u8; 64];
        hdr[0..4].copy_from_slice(&ELFMAG);
        hdr[4] = ELFCLASS64;
        hdr[5] = ELFDATA2LSB;
        hdr[16..18].copy_from_slice(&etype.to_le_bytes());
        hdr[18..20].copy_from_slice(&EM_X86_64.to_le_bytes());
        hdr[24..32].copy_from_slice(&entry.to_le_bytes());
        hdr[32..40].copy_from_slice(&phoff.to_le_bytes());
        hdr[56..58].copy_from_slice(&phnum.to_le_bytes());
        hdr
    }

    #[test]
    fn test_parse_header_valid_exec() {
        let hdr = build_elf_header(0x400000, 64, 1, ET_EXEC);
        let result = parse_header(&hdr).unwrap();
        assert_eq!(result.entry, 0x400000);
        assert_eq!(result.phoff, 64);
        assert_eq!(result.phnum, 1);
    }

    #[test]
    fn test_parse_header_valid_dyn() {
        let hdr = build_elf_header(0x1000, 64, 0, ET_DYN);
        let result = parse_header(&hdr).unwrap();
        assert_eq!(result.entry, 0x1000);
    }

    #[test]
    fn test_parse_header_too_small() {
        assert_eq!(parse_header(&[0u8; 32]), Err(ElfError::BadMagic));
    }

    #[test]
    fn test_parse_header_bad_magic() {
        let mut hdr = build_elf_header(0, 64, 0, ET_EXEC);
        hdr[0] = 0x00; // corrupt magic
        assert_eq!(parse_header(&hdr), Err(ElfError::BadMagic));
    }

    #[test]
    fn test_parse_header_not_64bit() {
        let mut hdr = build_elf_header(0, 64, 0, ET_EXEC);
        hdr[4] = 1; // ELFCLASS32
        assert_eq!(parse_header(&hdr), Err(ElfError::NotElf64));
    }

    #[test]
    fn test_parse_header_not_le() {
        let mut hdr = build_elf_header(0, 64, 0, ET_EXEC);
        hdr[5] = 2; // big-endian
        assert_eq!(parse_header(&hdr), Err(ElfError::NotLittleEndian));
    }

    #[test]
    fn test_parse_header_wrong_arch() {
        let mut hdr = build_elf_header(0, 64, 0, ET_EXEC);
        hdr[18] = 0x03; // EM_386
        hdr[19] = 0x00;
        assert_eq!(parse_header(&hdr), Err(ElfError::WrongArchitecture));
    }

    #[test]
    fn test_parse_header_not_executable() {
        let hdr = build_elf_header(0, 64, 0, 0); // ET_NONE
        assert_eq!(parse_header(&hdr), Err(ElfError::NotExecutable));
    }

    #[test]
    fn test_parse_program_header() {
        // Build a minimal ELF with one PT_LOAD program header.
        let mut data = build_elf_header(0x401000, 64, 1, ET_EXEC);
        // Extend to fit one program header (56 bytes).
        data.resize(64 + 56, 0);
        let phoff = 64u64;

        // Set p_type = PT_LOAD at offset 0 in the program header.
        data[64..68].copy_from_slice(&PT_LOAD.to_le_bytes());
        // Set p_vaddr = 0x400000.
        data[80..88].copy_from_slice(&0x400000u64.to_le_bytes());
        // Set p_filesz = 256.
        data[96..104].copy_from_slice(&256u64.to_le_bytes());
        // Set p_memsz = 512.
        data[104..112].copy_from_slice(&512u64.to_le_bytes());

        let ph = parse_program_header(&data, phoff, 0).unwrap();
        assert_eq!(ph.p_type, PT_LOAD);
        assert_eq!(ph.p_vaddr, 0x400000);
        assert_eq!(ph.p_filesz, 256);
        assert_eq!(ph.p_memsz, 512);
    }

    #[test]
    fn test_parse_program_header_out_of_bounds() {
        let data = build_elf_header(0, 64, 0, ET_EXEC);
        assert_eq!(
            parse_program_header(&data, 64, 0),
            Err(ElfError::InvalidSegment)
        );
    }

    #[test]
    fn test_elf_error_display() {
        // Ensure errors are Debug/Clone/Copy.
        let e = ElfError::BadMagic;
        let e2 = e;
        assert_eq!(e, e2);
        assert_eq!(format!("{:?}", e), "BadMagic");
    }
}
