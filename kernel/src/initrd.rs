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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
///
/// # Errors
///
/// Returns `InitrdError::TooSmall` if the data is too short for a header.
/// Returns `InitrdError::BadMagic` if the magic number is invalid.
///
/// # Panics
///
/// Panics if internal byte slice conversions fail (should not happen with valid data).
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
///
/// # Errors
///
/// Returns `InitrdError` if the index is out of bounds or data is invalid.
///
/// # Panics
///
/// Panics if internal byte slice conversions fail (should not happen with valid data).
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
#[must_use]
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid initrd archive for testing.
    fn build_test_archive(files: &[(&str, &[u8])]) -> Vec<u8> {
        let count = files.len() as u32;
        let table_size = 8 + (files.len() * ENTRY_SIZE);
        let data_offset = (table_size + 4095) & !4095; // page-align

        let total_size = data_offset + files.iter().map(|(_, d)| d.len()).sum::<usize>();
        let mut archive = vec![0u8; total_size];

        // Header.
        archive[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        archive[4..8].copy_from_slice(&count.to_le_bytes());

        // File entries and data.
        let mut current_offset = data_offset;
        for (i, (name, data)) in files.iter().enumerate() {
            let entry_start = 8 + i * ENTRY_SIZE;

            // Name (null-terminated, padded to 256 bytes).
            let name_bytes = name.as_bytes();
            archive[entry_start..entry_start + name_bytes.len()].copy_from_slice(name_bytes);
            // Null terminator is already 0 from vec init.

            // Offset and size.
            archive[entry_start + 256..entry_start + 260]
                .copy_from_slice(&(current_offset as u32).to_le_bytes());
            archive[entry_start + 260..entry_start + 264]
                .copy_from_slice(&(data.len() as u32).to_le_bytes());

            // File data.
            archive[current_offset..current_offset + data.len()].copy_from_slice(data);
            current_offset += data.len();
        }

        archive
    }

    #[test]
    fn test_parse_header_valid() {
        let archive = build_test_archive(&[("test.txt", b"hello")]);
        let count = parse_header(&archive).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_parse_header_too_small() {
        assert_eq!(parse_header(&[0u8; 4]), Err(InitrdError::TooSmall));
    }

    #[test]
    fn test_parse_header_bad_magic() {
        let mut data = vec![0u8; 8];
        data[0..4].copy_from_slice(&0xDEADBEEFu32.to_le_bytes());
        assert_eq!(parse_header(&data), Err(InitrdError::BadMagic));
    }

    #[test]
    fn test_get_file_valid() {
        let archive = build_test_archive(&[("hello.txt", b"world")]);
        let file = get_file(&archive, 0).unwrap();
        assert_eq!(file.name, "hello.txt");
        assert_eq!(file.data, b"world");
    }

    #[test]
    fn test_get_file_index_out_of_bounds() {
        let archive = build_test_archive(&[("a.txt", b"data")]);
        assert_eq!(get_file(&archive, 1), Err(InitrdError::IndexOutOfBounds));
    }

    #[test]
    fn test_find_file_found() {
        let archive = build_test_archive(&[("file1.txt", b"aaa"), ("file2.txt", b"bbb")]);
        let file = find_file(&archive, "file2.txt").unwrap();
        assert_eq!(file.data, b"bbb");
    }

    #[test]
    fn test_find_file_not_found() {
        let archive = build_test_archive(&[("file1.txt", b"aaa")]);
        assert!(find_file(&archive, "missing.txt").is_none());
    }

    #[test]
    fn test_multiple_files() {
        let archive = build_test_archive(&[
            ("a.elf", b"\x7fELF"),
            ("b.bin", &[0xDE, 0xAD, 0xBE, 0xEF]),
            ("c.txt", b"hello world"),
        ]);
        assert_eq!(get_file(&archive, 0).unwrap().name, "a.elf");
        assert_eq!(get_file(&archive, 1).unwrap().name, "b.bin");
        assert_eq!(get_file(&archive, 2).unwrap().name, "c.txt");
        assert_eq!(get_file(&archive, 2).unwrap().data, b"hello world");
    }

    #[test]
    fn test_empty_archive() {
        let archive = build_test_archive(&[]);
        assert_eq!(parse_header(&archive).unwrap(), 0);
    }
}
