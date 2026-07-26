//! Initrd (initial ramdisk) parser.
//!
//! Parses a custom binary archive format and provides file lookup by name.
//! The initrd is loaded by the bootloader and passed to the kernel via
//! `BootInfo.ramdisk_addr` and `BootInfo.ramdisk_len`.
//!
//! ## Archive Format
//!
//! ```text
//! Header (8 bytes):
//!   [0x00] magic:   u32 = 0x4F535244  ("OSRD" little-endian)
//!   [0x04] count:   u32               number of files
//!
//! File entries (268 bytes each):
//!   [0x00]   name:    [u8; 256]       null-terminated filename
//!   [0x100]  offset:  u32             byte offset from start of archive
//!   [0x104]  size:    u32             file size in bytes
//!
//! Data section:
//!   Raw file contents at the specified offsets.
//! ```

/// Magic number identifying an `OpenOS` ramdisk archive: "OSRD" in little-endian.
const MAGIC: u32 = 0x4F53_5244;

/// Size of a file entry in bytes (256 + 4 + 4 + 4 padding = 268).
const ENTRY_SIZE: usize = 264;

/// Maximum filename length (including null terminator).
const NAME_MAX: usize = 256;

/// A file found in the initrd archive.
#[derive(Debug, Clone, Copy)]
pub struct InitrdFile<'a> {
    /// File name (null-terminated, trimmed).
    pub name: &'a str,
    /// File contents.
    pub data: &'a [u8],
}

/// Errors that can occur when parsing the initrd.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitrdError {
    /// Archive is too small to contain a valid header.
    TooSmall,
    /// Bad magic number — not an `OpenOS` ramdisk.
    BadMagic,
    /// File index out of bounds.
    IndexOutOfBounds,
    /// Filename is not valid UTF-8.
    InvalidFilename,
    /// File data extends beyond the archive.
    DataOutOfBounds,
}

/// Parse the initrd header and return the file count.
///
/// Validates the magic number and minimum size.
pub fn parse_header(data: &[u8]) -> Result<u32, InitrdError> {
    if data.len() < 8 {
        return Err(InitrdError::TooSmall);
    }
    let magic = u32::from_le_bytes(data[0..4].try_into().unwrap());
    if magic != MAGIC {
        return Err(InitrdError::BadMagic);
    }
    let count = u32::from_le_bytes(data[4..8].try_into().unwrap());
    Ok(count)
}

/// Get a file entry from the archive by index.
///
/// Returns the file name and data slice.
pub fn get_file(data: &[u8], index: u32) -> Result<InitrdFile<'_>, InitrdError> {
    let count = parse_header(data)?;
    if index >= count {
        return Err(InitrdError::IndexOutOfBounds);
    }

    let entry_offset = 8 + (index as usize) * ENTRY_SIZE;
    if entry_offset + ENTRY_SIZE > data.len() {
        return Err(InitrdError::TooSmall);
    }

    let entry = &data[entry_offset..entry_offset + ENTRY_SIZE];

    // Extract filename (null-terminated).
    let name_bytes = &entry[0..NAME_MAX];
    let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(NAME_MAX);
    let name =
        core::str::from_utf8(&name_bytes[..name_len]).map_err(|_| InitrdError::InvalidFilename)?;

    // Extract offset and size.
    let file_offset = u32::from_le_bytes(entry[256..260].try_into().unwrap()) as usize;
    let file_size = u32::from_le_bytes(entry[260..264].try_into().unwrap()) as usize;

    // Bounds check.
    if file_offset + file_size > data.len() {
        return Err(InitrdError::DataOutOfBounds);
    }

    Ok(InitrdFile {
        name,
        data: &data[file_offset..file_offset + file_size],
    })
}

/// Find a file by name in the initrd archive.
///
/// Returns `None` if no file with the given name exists.
pub fn find_file<'a>(data: &'a [u8], target: &str) -> Option<InitrdFile<'a>> {
    let count = parse_header(data).ok()?;
    for i in 0..count {
        if let Ok(file) = get_file(data, i) {
            if file.name == target {
                return Some(file);
            }
        }
    }
    None
}
