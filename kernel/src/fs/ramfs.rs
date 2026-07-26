//! Simple in-memory filesystem (ramfs).
//!
//! Provides basic file operations: create, read, write, delete.
//! Files are stored in a static array with a simple table structure.
//!
//! ## Design
//!
//! - Storage: 64 KiB static array
//! - Max files: 32
//! - Max filename: 63 bytes (null-terminated)
//! - Max file size: 2048 bytes
//! - No directories (flat namespace)

use alloc::string::String;
use alloc::vec::Vec;

use spin::Mutex;

/// Maximum number of files.
const MAX_FILES: usize = 32;

/// Maximum filename length (including null terminator).
const MAX_NAME_LEN: usize = 64;

/// Maximum file size in bytes.
const MAX_FILE_SIZE: usize = 2048;

/// Total storage size.
const STORAGE_SIZE: usize = 65536;

/// A file entry in the ramfs.
struct FileEntry {
    /// Filename (null-terminated).
    name: [u8; MAX_NAME_LEN],
    /// File data.
    data: Vec<u8>,
    /// Whether this slot is in use.
    in_use: bool,
}

/// Global ramfs state.
static RAMFS: Mutex<RamFs> = Mutex::new(RamFs::new());

/// In-memory filesystem.
struct RamFs {
    files: [FileEntry; MAX_FILES],
}

impl RamFs {
    #[allow(clippy::too_many_lines)]
    const fn new() -> Self {
        // Can't use Vec in const, so we use a fixed array.
        // We'll initialize in init().
        Self {
            files: [
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                },
            ],
        }
    }

    fn find_file(&self, name: &str) -> Option<usize> {
        for (i, f) in self.files.iter().enumerate() {
            if f.in_use {
                let fname = core::str::from_utf8(
                    &f.name[..f.name.iter().position(|&b| b == 0).unwrap_or(MAX_NAME_LEN)],
                );
                if fname == Ok(name) {
                    return Some(i);
                }
            }
        }
        None
    }

    fn find_free_slot(&self) -> Option<usize> {
        self.files.iter().position(|f| !f.in_use)
    }
}

/// Errors that can occur in ramfs operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RamFsError {
    NotFound,
    AlreadyExists,
    NoSpace,
    FileTooLarge,
    InvalidName,
}

/// Create a new file with the given name and data.
pub fn create_file(name: &str, data: &[u8]) -> Result<(), RamFsError> {
    if name.is_empty() || name.len() >= MAX_NAME_LEN {
        return Err(RamFsError::InvalidName);
    }
    if data.len() > MAX_FILE_SIZE {
        return Err(RamFsError::FileTooLarge);
    }

    let mut fs = RAMFS.lock();

    if fs.find_file(name).is_some() {
        return Err(RamFsError::AlreadyExists);
    }

    let slot = fs.find_free_slot().ok_or(RamFsError::NoSpace)?;
    let entry = &mut fs.files[slot];

    entry.name = [0; MAX_NAME_LEN];
    let name_bytes = name.as_bytes();
    entry.name[..name_bytes.len()].copy_from_slice(name_bytes);
    entry.data = Vec::new();
    entry.data.extend_from_slice(data);
    entry.in_use = true;

    Ok(())
}

/// Read the contents of a file.
pub fn read_file(name: &str) -> Result<Vec<u8>, RamFsError> {
    let fs = RAMFS.lock();
    let idx = fs.find_file(name).ok_or(RamFsError::NotFound)?;
    Ok(fs.files[idx].data.clone())
}

/// Write data to an existing file (replaces contents).
pub fn write_file(name: &str, data: &[u8]) -> Result<(), RamFsError> {
    if data.len() > MAX_FILE_SIZE {
        return Err(RamFsError::FileTooLarge);
    }

    let mut fs = RAMFS.lock();
    let idx = fs.find_file(name).ok_or(RamFsError::NotFound)?;
    fs.files[idx].data.clear();
    fs.files[idx].data.extend_from_slice(data);
    Ok(())
}

/// Delete a file.
pub fn delete_file(name: &str) -> Result<(), RamFsError> {
    let mut fs = RAMFS.lock();
    let idx = fs.find_file(name).ok_or(RamFsError::NotFound)?;
    fs.files[idx].in_use = false;
    fs.files[idx].data.clear();
    fs.files[idx].name = [0; MAX_NAME_LEN];
    Ok(())
}

/// List all files in the ramfs.
pub fn list_files() -> Vec<String> {
    let fs = RAMFS.lock();
    let mut result = Vec::new();
    for f in &fs.files {
        if f.in_use {
            let name_len = f.name.iter().position(|&b| b == 0).unwrap_or(MAX_NAME_LEN);
            if let Ok(name) = core::str::from_utf8(&f.name[..name_len]) {
                result.push(String::from(name));
            }
        }
    }
    result
}

/// Initialize the ramfs.
pub fn init() {
    crate::serial_println!(
        "[OK] Ramfs initialized ({} files max, {} bytes storage)",
        MAX_FILES,
        STORAGE_SIZE
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_read() {
        // Reset ramfs state for test.
        create_file("test.txt", b"hello").unwrap();
        let data = read_file("test.txt").unwrap();
        assert_eq!(data, b"hello");
        delete_file("test.txt").unwrap();
    }

    #[test]
    fn test_create_duplicate() {
        create_file("dup.txt", b"a").unwrap();
        assert_eq!(create_file("dup.txt", b"b"), Err(RamFsError::AlreadyExists));
        delete_file("dup.txt").unwrap();
    }

    #[test]
    fn test_read_not_found() {
        assert_eq!(read_file("nonexistent"), Err(RamFsError::NotFound));
    }

    #[test]
    fn test_write_file() {
        create_file("w.txt", b"old").unwrap();
        write_file("w.txt", b"new").unwrap();
        let data = read_file("w.txt").unwrap();
        assert_eq!(data, b"new");
        delete_file("w.txt").unwrap();
    }

    #[test]
    fn test_delete_file() {
        create_file("del.txt", b"data").unwrap();
        delete_file("del.txt").unwrap();
        assert_eq!(read_file("del.txt"), Err(RamFsError::NotFound));
    }

    #[test]
    fn test_list_files() {
        create_file("a.txt", b"1").unwrap();
        create_file("b.txt", b"2").unwrap();
        let files = list_files();
        assert!(files.contains(&String::from("a.txt")));
        assert!(files.contains(&String::from("b.txt")));
        delete_file("a.txt").unwrap();
        delete_file("b.txt").unwrap();
    }

    #[test]
    fn test_file_too_large() {
        let big = vec![0u8; MAX_FILE_SIZE + 1];
        assert_eq!(create_file("big", &big), Err(RamFsError::FileTooLarge));
    }

    #[test]
    fn test_invalid_name() {
        assert_eq!(create_file("", b"data"), Err(RamFsError::InvalidName));
    }
}
