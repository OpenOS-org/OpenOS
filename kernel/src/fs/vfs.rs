//! Virtual File System (VFS) trait and supporting types.
//!
//! Defines the abstract interface that all filesystem implementations must
//! satisfy. In a microkernel design, the VFS server runs in user-space and
//! dispatches file operations to the appropriate filesystem driver over IPC.
//!
//! This module provides the kernel-side trait definitions that both in-kernel
//! ramfs and future user-space filesystem servers implement.

use alloc::string::String;
use alloc::vec::Vec;

/// Errors returned by filesystem operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsError {
    /// The file or directory was not found.
    NotFound,
    /// The file already exists (on create).
    AlreadyExists,
    /// Permission denied for the requested operation.
    PermissionDenied,
    /// No space left on the filesystem.
    NoSpace,
    /// The file name is invalid (empty, too long, etc.).
    InvalidName,
    /// The file is too large.
    FileTooLarge,
    /// The file descriptor is invalid.
    BadFileDescriptor,
    /// The operation is not supported by this filesystem.
    NotSupported,
    /// An I/O error occurred.
    IoError,
}

/// Metadata for an inode (file or directory).
#[derive(Debug, Clone)]
pub struct InodeMeta {
    /// Inode number (unique within the filesystem).
    pub ino: u64,
    /// File type: `true` = directory, `false` = regular file.
    pub is_dir: bool,
    /// File size in bytes.
    pub size: u64,
    /// Number of hard links.
    pub nlink: u32,
}

/// A directory entry returned by `readdir`.
#[derive(Debug, Clone)]
pub struct DirEntry {
    /// The entry name (not a full path).
    pub name: String,
    /// The inode number of this entry.
    pub ino: u64,
    /// Whether this entry is a directory.
    pub is_dir: bool,
}

/// An open file descriptor, tracking the inode and current offset.
#[derive(Debug, Clone)]
pub struct FileDescriptor {
    /// The inode number this descriptor refers to.
    pub ino: u64,
    /// Current read/write offset within the file.
    pub offset: u64,
    /// Open flags (e.g., read-only, write-only, read-write).
    pub flags: OpenFlags,
}

/// Flags for opening a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenFlags(u32);

#[allow(dead_code)]
impl OpenFlags {
    /// Create the file if it does not exist.
    pub const CREATE: Self = Self(1 << 2);
    /// Open for reading only.
    pub const READ: Self = Self(1 << 0);
    /// Open for reading and writing.
    pub const READ_WRITE: Self = Self(Self::READ.0 | Self::WRITE.0);
    /// Truncate the file to zero length on open.
    pub const TRUNCATE: Self = Self(1 << 3);
    /// Open for writing only.
    pub const WRITE: Self = Self(1 << 1);

    pub fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    pub fn contains(self, flag: Self) -> bool {
        (self.0 & flag.0) == flag.0
    }

    pub fn raw(self) -> u32 {
        self.0
    }
}

/// Abstract filesystem interface.
///
/// Implementations provide the backing store for files and directories.
/// The VFS layer dispatches operations through this trait.
pub trait FileSystem {
    /// Open a file by path. Returns an inode number.
    fn open(&self, path: &str, flags: OpenFlags) -> Result<u64, FsError>;

    /// Close an open file descriptor.
    fn close(&self, ino: u64) -> Result<(), FsError>;

    /// Read bytes from an inode at the given offset.
    ///
    /// Returns the number of bytes read, or 0 at EOF.
    fn read(&self, ino: u64, offset: u64, buf: &mut [u8]) -> Result<usize, FsError>;

    /// Write bytes to an inode at the given offset.
    ///
    /// Returns the number of bytes written.
    fn write(&self, ino: u64, offset: u64, data: &[u8]) -> Result<usize, FsError>;

    /// Get metadata for an inode.
    fn stat(&self, ino: u64) -> Result<InodeMeta, FsError>;

    /// List entries in a directory inode.
    fn readdir(&self, dir_ino: u64) -> Result<Vec<DirEntry>, FsError>;

    /// Create a new file. Returns the new inode number.
    fn create(&self, parent_ino: u64, name: &str) -> Result<u64, FsError>;

    /// Remove a file by name from a directory.
    fn unlink(&self, parent_ino: u64, name: &str) -> Result<(), FsError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_flags_read() {
        let flags = OpenFlags::READ;
        assert!(flags.contains(OpenFlags::READ));
        assert!(!flags.contains(OpenFlags::WRITE));
    }

    #[test]
    fn test_open_flags_read_write() {
        let flags = OpenFlags::READ_WRITE;
        assert!(flags.contains(OpenFlags::READ));
        assert!(flags.contains(OpenFlags::WRITE));
    }

    #[test]
    fn test_open_flags_create() {
        let flags = OpenFlags::CREATE;
        assert!(flags.contains(OpenFlags::CREATE));
        assert!(!flags.contains(OpenFlags::READ));
    }

    #[test]
    fn test_open_flags_raw_roundtrip() {
        let flags = OpenFlags::READ_WRITE | OpenFlags::CREATE;
        let raw = flags.raw();
        let restored = OpenFlags::from_raw(raw);
        assert_eq!(flags, restored);
    }

    #[test]
    fn test_fs_error_variants_unique() {
        let errors = [
            FsError::NotFound,
            FsError::AlreadyExists,
            FsError::PermissionDenied,
            FsError::NoSpace,
            FsError::InvalidName,
            FsError::FileTooLarge,
            FsError::BadFileDescriptor,
            FsError::NotSupported,
            FsError::IoError,
        ];
        for i in 0..errors.len() {
            for j in (i + 1)..errors.len() {
                assert_ne!(errors[i], errors[j]);
            }
        }
    }

    #[test]
    fn test_inode_meta_clone() {
        let meta = InodeMeta {
            ino: 42,
            is_dir: false,
            size: 1024,
            nlink: 1,
        };
        let cloned = meta.clone();
        assert_eq!(cloned.ino, 42);
        assert!(!cloned.is_dir);
        assert_eq!(cloned.size, 1024);
    }

    #[test]
    fn test_dir_entry_clone() {
        let entry = DirEntry {
            name: alloc::string::String::from("test.txt"),
            ino: 7,
            is_dir: false,
        };
        let cloned = entry.clone();
        assert_eq!(cloned.name, "test.txt");
        assert_eq!(cloned.ino, 7);
    }

    #[test]
    fn test_file_descriptor_clone() {
        let fd = FileDescriptor {
            ino: 10,
            offset: 0,
            flags: OpenFlags::READ_WRITE,
        };
        let cloned = fd.clone();
        assert_eq!(cloned.ino, 10);
        assert_eq!(cloned.offset, 0);
    }
}
