//! Virtual File System (VFS) trait and supporting types.
//!
//! Defines the abstract interface that all filesystem implementations must
//! satisfy. In a microkernel design, the VFS server runs in user-space and
//! dispatches file operations to the appropriate filesystem driver over IPC.
//!
//! This module provides the kernel-side trait definitions that both in-kernel
//! ramfs and future user-space filesystem servers implement.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ops::BitOr;

use spin::Mutex;

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
    /// The target is not a directory.
    NotADirectory,
}

/// Metadata for an inode (file, directory, or symlink).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InodeMeta {
    /// Inode number (unique within the filesystem).
    pub ino: u64,
    /// File type: `true` = directory, `false` = regular file.
    pub is_dir: bool,
    /// Whether this inode is a symbolic link.
    pub is_symlink: bool,
    /// File size in bytes.
    pub size: u64,
    /// Number of hard links.
    pub nlink: u32,
}

/// A directory entry returned by `readdir`.
#[derive(Debug, Clone, PartialEq, Eq)]
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

    /// Create flags from a raw `u32` value.
    #[must_use]
    pub fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// Check if the flags contain the given flag.
    #[must_use]
    pub fn contains(self, flag: Self) -> bool {
        (self.0 & flag.0) == flag.0
    }

    /// Get the raw `u32` value of the flags.
    #[must_use]
    pub fn raw(self) -> u32 {
        self.0
    }
}

impl BitOr for OpenFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// Abstract filesystem interface.
///
/// Implementations provide the backing store for files and directories.
/// The VFS layer dispatches operations through this trait.
#[allow(clippy::missing_errors_doc)]
pub trait FileSystem: Send + Sync {
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

    /// Create a symbolic link.
    ///
    /// Creates a new directory entry `link_path` in the directory `parent_ino`
    /// that points to `target`. The target is stored as an opaque string and
    /// is not resolved by the filesystem.
    ///
    /// # Errors
    ///
    /// Returns `FsError::NotSupported` if the filesystem does not support symlinks.
    fn symlink(&self, parent_ino: u64, name: &str, target: &str) -> Result<u64, FsError>;

    /// Read the target of a symbolic link.
    ///
    /// Returns the number of bytes written to `buf`, or an error.
    ///
    /// # Errors
    ///
    /// Returns `FsError::NotSupported` if the inode is not a symlink.
    fn readlink(&self, ino: u64, buf: &mut [u8]) -> Result<usize, FsError>;

    /// Create a new directory.
    ///
    /// Creates a directory entry `name` in the parent directory `parent_ino`.
    /// Returns the inode number of the newly created directory.
    ///
    /// # Errors
    ///
    /// Returns `FsError::NotSupported` if the filesystem does not support directories.
    /// Returns `FsError::AlreadyExists` if `name` already exists in the parent.
    /// Returns `FsError::NotFound` if `parent_ino` is not a valid directory.
    fn mkdir(&self, parent_ino: u64, name: &str) -> Result<u64, FsError> {
        let _ = (parent_ino, name);
        Err(FsError::NotSupported)
    }

    /// Remove an empty directory.
    ///
    /// Removes the directory entry `name` from the parent directory `parent_ino`.
    /// The directory must be empty (contain no entries other than `.` and `..`).
    ///
    /// # Errors
    ///
    /// Returns `FsError::NotSupported` if the filesystem does not support directories.
    /// Returns `FsError::NotFound` if `name` does not exist in the parent.
    /// Returns `FsError::NotADirectory` if `name` refers to a non-directory.
    fn rmdir(&self, parent_ino: u64, name: &str) -> Result<(), FsError> {
        let _ = (parent_ino, name);
        Err(FsError::NotSupported)
    }
}

// ---------------------------------------------------------------------------
// Mount table — path-based filesystem dispatch
// ---------------------------------------------------------------------------

/// A mount point binding a path prefix to a filesystem instance.
pub struct MountPoint {
    /// The path prefix this filesystem is mounted at (e.g., "/", "/disk").
    pub path: String,
    /// Block device index (for informational purposes; 0 if not applicable).
    pub device_idx: usize,
    /// The filesystem instance serving this mount point.
    pub fs: Arc<dyn FileSystem>,
}

/// Global mount table. Maps path prefixes to filesystem instances.
///
/// Entries are ordered by insertion; `resolve_fs` selects the longest
/// matching prefix.
static MOUNT_TABLE: Mutex<Vec<MountPoint>> = Mutex::new(Vec::new());

/// Mount a filesystem at the given path prefix.
///
/// The path must start with '/'. Registers an entry in the global mount table.
///
/// # Errors
///
/// Returns `Err(())` if the path is empty, does not start with '/', or is already mounted.
pub fn mount(path: &str, device_idx: usize, fs: Arc<dyn FileSystem>) -> Result<(), ()> {
    if path.is_empty() || !path.starts_with('/') {
        return Err(());
    }

    let mut table = MOUNT_TABLE.lock();

    // Prevent duplicate mounts at the same path.
    if table.iter().any(|mp| mp.path == path) {
        return Err(());
    }

    table.push(MountPoint {
        path: String::from(path),
        device_idx,
        fs,
    });

    crate::serial_println!("[VFS] Mounted filesystem at '{}'", path);
    Ok(())
}

/// Unmount the filesystem at the given path prefix.
///
/// # Errors
///
/// Returns `Err(())` if no filesystem is mounted at the specified path.
pub fn unmount(path: &str) -> Result<(), ()> {
    let mut table = MOUNT_TABLE.lock();
    let len_before = table.len();
    table.retain(|mp| mp.path != path);
    if table.len() == len_before {
        return Err(());
    }
    crate::serial_println!("[VFS] Unmounted filesystem at '{}'", path);
    Ok(())
}

/// Resolve a path to its backing filesystem and the relative path within it.
///
/// Finds the mount point with the longest matching prefix. For example,
/// if "/" and "/disk" are mounted, a path of "/disk/foo" resolves to
/// the "/disk" filesystem with relative path "foo".
///
/// # Returns
///
/// `(Arc<dyn FileSystem>, relative_path)` — the filesystem instance and
/// the path relative to the mount point (with leading '/' stripped).
///
/// # Panics
///
/// Panics if no filesystem is mounted (no "/" root mount). This should
/// never happen in normal operation since the root is mounted at boot.
pub fn resolve_fs(path: &str) -> (Arc<dyn FileSystem>, String) {
    let table = MOUNT_TABLE.lock();

    let mut best_match: Option<&MountPoint> = None;
    let mut best_len: usize = 0;

    for mp in table.iter() {
        // Check if `path` starts with this mount point's prefix.
        if path.starts_with(&mp.path[..]) && mp.path.len() > best_len {
            best_match = Some(mp);
            best_len = mp.path.len();
        }
    }

    let mp = best_match.expect("no root filesystem mounted at '/'");

    // Compute the relative path: strip the mount prefix.
    let relative = if path.len() > best_len {
        // Strip the leading '/' from the relative portion if present.
        let rest = &path[best_len..];
        rest.strip_prefix('/').unwrap_or(rest)
    } else {
        ""
    };

    (Arc::clone(&mp.fs), String::from(relative))
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
            FsError::NotADirectory,
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
            is_symlink: false,
            size: 1024,
            nlink: 1,
        };
        let cloned = meta.clone();
        assert_eq!(cloned.ino, 42);
        assert!(!cloned.is_dir);
        assert!(!cloned.is_symlink);
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

    // --- Mount point tests ---

    /// Minimal mock filesystem for testing mount/unmount/resolve.
    struct MockFs;
    impl FileSystem for MockFs {
        fn open(&self, _path: &str, _flags: OpenFlags) -> Result<u64, FsError> {
            Ok(1)
        }

        fn close(&self, _ino: u64) -> Result<(), FsError> {
            Ok(())
        }

        fn read(&self, _ino: u64, _offset: u64, _buf: &mut [u8]) -> Result<usize, FsError> {
            Ok(0)
        }

        fn write(&self, _ino: u64, _offset: u64, _data: &[u8]) -> Result<usize, FsError> {
            Ok(0)
        }

        fn stat(&self, _ino: u64) -> Result<InodeMeta, FsError> {
            Ok(InodeMeta {
                ino: 1,
                is_dir: false,
                is_symlink: false,
                size: 0,
                nlink: 1,
            })
        }

        fn readdir(&self, _dir_ino: u64) -> Result<Vec<DirEntry>, FsError> {
            Ok(Vec::new())
        }

        fn create(&self, _parent_ino: u64, _name: &str) -> Result<u64, FsError> {
            Ok(1)
        }

        fn unlink(&self, _parent_ino: u64, _name: &str) -> Result<(), FsError> {
            Ok(())
        }

        fn symlink(&self, _parent_ino: u64, _name: &str, _target: &str) -> Result<u64, FsError> {
            Err(FsError::NotSupported)
        }

        fn readlink(&self, _ino: u64, _buf: &mut [u8]) -> Result<usize, FsError> {
            Err(FsError::NotSupported)
        }

        fn mkdir(&self, _parent_ino: u64, _name: &str) -> Result<u64, FsError> {
            Err(FsError::NotSupported)
        }

        fn rmdir(&self, _parent_ino: u64, _name: &str) -> Result<(), FsError> {
            Err(FsError::NotSupported)
        }
    }

    #[test]
    fn test_mount_and_resolve_root() {
        let fs: Arc<dyn FileSystem> = Arc::new(MockFs);
        assert!(mount("/", 0, Arc::clone(&fs)).is_ok());

        let (resolved, rel) = resolve_fs("/hello.txt");
        // resolved should be the same Arc.
        assert!(Arc::ptr_eq(&resolved, &fs));
        assert_eq!(rel, "hello.txt");

        unmount("/").unwrap();
    }

    #[test]
    fn test_mount_subpath_and_resolve() {
        // Clean up any existing mounts from other tests.
        let _ = unmount("/disk");
        let _ = unmount("/");
        let root_fs: Arc<dyn FileSystem> = Arc::new(MockFs);
        let disk_fs: Arc<dyn FileSystem> = Arc::new(MockFs);
        mount("/", 0, Arc::clone(&root_fs)).unwrap();
        mount("/disk", 1, Arc::clone(&disk_fs)).unwrap();

        // Path under /disk should resolve to disk_fs.
        let (resolved, rel) = resolve_fs("/disk/data.bin");
        assert!(Arc::ptr_eq(&resolved, &disk_fs));
        assert_eq!(rel, "data.bin");

        // Root-level path should resolve to root_fs.
        let (resolved2, rel2) = resolve_fs("/etc/config");
        assert!(Arc::ptr_eq(&resolved2, &root_fs));
        assert_eq!(rel2, "etc/config");

        unmount("/disk").unwrap();
        unmount("/").unwrap();
    }

    #[test]
    fn test_mount_exact_path_returns_empty_relative() {
        let _ = unmount("/disk");
        let _ = unmount("/");
        let fs: Arc<dyn FileSystem> = Arc::new(MockFs);
        mount("/", 0, Arc::clone(&fs)).unwrap();

        let (_, rel) = resolve_fs("/");
        assert_eq!(rel, "");

        unmount("/").unwrap();
    }

    #[test]
    fn test_mount_duplicate_fails() {
        let _ = unmount("/disk");
        let _ = unmount("/");
        let fs1: Arc<dyn FileSystem> = Arc::new(MockFs);
        let fs2: Arc<dyn FileSystem> = Arc::new(MockFs);
        assert!(mount("/", 0, fs1).is_ok());
        assert!(mount("/", 0, fs2).is_err());

        unmount("/").unwrap();
    }

    #[test]
    fn test_mount_invalid_path_fails() {
        let fs: Arc<dyn FileSystem> = Arc::new(MockFs);
        assert!(mount("", 0, Arc::clone(&fs)).is_err());
        assert!(mount("no_slash", 0, Arc::clone(&fs)).is_err());
    }

    #[test]
    fn test_unmount_nonexistent_fails() {
        assert!(unmount("/nonexistent").is_err());
    }

    // --- mkdir / rmdir trait default tests ---

    #[test]
    fn test_mock_mkdir_not_supported() {
        let fs = MockFs;
        assert_eq!(fs.mkdir(0, "test"), Err(FsError::NotSupported));
    }

    #[test]
    fn test_mock_rmdir_not_supported() {
        let fs = MockFs;
        assert_eq!(fs.rmdir(0, "test"), Err(FsError::NotSupported));
    }

    #[test]
    fn test_not_adirectory_error_unique() {
        assert_ne!(FsError::NotADirectory, FsError::NotFound);
        assert_ne!(FsError::NotADirectory, FsError::NotSupported);
        assert_ne!(FsError::NotADirectory, FsError::InvalidName);
    }

    // --- Resolve FS tests (requested) ---

    #[test]
    fn test_resolve_fs_root_mount() {
        // Mount at "/" and verify resolution returns correct fs and relative path.
        let fs: Arc<dyn FileSystem> = Arc::new(MockFs);
        mount("/", 0, Arc::clone(&fs)).unwrap();

        let (resolved, rel) = resolve_fs("/hello.txt");
        assert!(Arc::ptr_eq(&resolved, &fs));
        assert_eq!(rel, "hello.txt");

        // Exact mount path returns empty relative.
        let (_, rel_exact) = resolve_fs("/");
        assert_eq!(rel_exact, "");

        // Deep path.
        let (_, rel_deep) = resolve_fs("/a/b/c.txt");
        assert_eq!(rel_deep, "a/b/c.txt");

        unmount("/").unwrap();
    }

    #[test]
    fn test_resolve_fs_nested_mount() {
        // Mount at "/" and "/disk", verify longest-prefix matching.
        let root_fs: Arc<dyn FileSystem> = Arc::new(MockFs);
        let disk_fs: Arc<dyn FileSystem> = Arc::new(MockFs);
        mount("/", 0, Arc::clone(&root_fs)).unwrap();
        mount("/disk", 1, Arc::clone(&disk_fs)).unwrap();

        // Path under /disk resolves to disk_fs.
        let (resolved, rel) = resolve_fs("/disk/data.bin");
        assert!(Arc::ptr_eq(&resolved, &disk_fs));
        assert_eq!(rel, "data.bin");

        // Path under /disk/subdir resolves to disk_fs.
        let (resolved2, rel2) = resolve_fs("/disk/sub/file.txt");
        assert!(Arc::ptr_eq(&resolved2, &disk_fs));
        assert_eq!(rel2, "sub/file.txt");

        // Root-level path resolves to root_fs.
        let (resolved3, rel3) = resolve_fs("/etc/config");
        assert!(Arc::ptr_eq(&resolved3, &root_fs));
        assert_eq!(rel3, "etc/config");

        // Exact /disk path returns empty relative.
        let (_, rel_exact) = resolve_fs("/disk");
        assert_eq!(rel_exact, "");

        unmount("/disk").unwrap();
        unmount("/").unwrap();
    }

    // --- OpenFlags tests (requested) ---

    #[test]
    fn test_open_flags_truncate() {
        let flags = OpenFlags::TRUNCATE;
        assert!(flags.contains(OpenFlags::TRUNCATE));
        assert!(!flags.contains(OpenFlags::READ));
        assert!(!flags.contains(OpenFlags::WRITE));
        assert!(!flags.contains(OpenFlags::CREATE));
    }

    #[test]
    fn test_open_flags_combined() {
        // READ | WRITE | CREATE | TRUNCATE
        let flags = OpenFlags::READ | OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE;
        assert!(flags.contains(OpenFlags::READ));
        assert!(flags.contains(OpenFlags::WRITE));
        assert!(flags.contains(OpenFlags::CREATE));
        assert!(flags.contains(OpenFlags::TRUNCATE));
    }

    #[test]
    fn test_open_flags_read_write_combined() {
        // READ_WRITE should contain both READ and WRITE.
        let flags = OpenFlags::READ_WRITE;
        assert!(flags.contains(OpenFlags::READ));
        assert!(flags.contains(OpenFlags::WRITE));
        assert!(!flags.contains(OpenFlags::CREATE));
        assert!(!flags.contains(OpenFlags::TRUNCATE));
    }

    #[test]
    fn test_open_flags_raw_zero() {
        let flags = OpenFlags::from_raw(0);
        assert!(!flags.contains(OpenFlags::READ));
        assert!(!flags.contains(OpenFlags::WRITE));
        assert!(!flags.contains(OpenFlags::CREATE));
        assert!(!flags.contains(OpenFlags::TRUNCATE));
        assert_eq!(flags.raw(), 0);
    }

    #[test]
    fn test_open_flags_bitor() {
        let a = OpenFlags::READ;
        let b = OpenFlags::CREATE;
        let combined = a | b;
        assert!(combined.contains(OpenFlags::READ));
        assert!(combined.contains(OpenFlags::CREATE));
        assert!(!combined.contains(OpenFlags::WRITE));
    }

    // --- Mount edge case tests ---

    #[test]
    fn test_unmount_and_remount() {
        // Unmount a path and remount with a different fs.
        let fs1: Arc<dyn FileSystem> = Arc::new(MockFs);
        let fs2: Arc<dyn FileSystem> = Arc::new(MockFs);
        mount("/", 0, Arc::clone(&fs1)).unwrap();

        let (resolved, _) = resolve_fs("/test");
        assert!(Arc::ptr_eq(&resolved, &fs1));

        unmount("/").unwrap();
        mount("/", 0, Arc::clone(&fs2)).unwrap();

        let (resolved2, _) = resolve_fs("/test");
        assert!(Arc::ptr_eq(&resolved2, &fs2));

        unmount("/").unwrap();
    }

    #[test]
    fn test_resolve_fs_multiple_mounts() {
        // Mount three filesystems and verify correct resolution.
        let root_fs: Arc<dyn FileSystem> = Arc::new(MockFs);
        let disk_fs: Arc<dyn FileSystem> = Arc::new(MockFs);
        let tmp_fs: Arc<dyn FileSystem> = Arc::new(MockFs);
        mount("/", 0, Arc::clone(&root_fs)).unwrap();
        mount("/disk", 1, Arc::clone(&disk_fs)).unwrap();
        mount("/tmp", 2, Arc::clone(&tmp_fs)).unwrap();

        let (r1, p1) = resolve_fs("/disk/file");
        assert!(Arc::ptr_eq(&r1, &disk_fs));
        assert_eq!(p1, "file");

        let (r2, p2) = resolve_fs("/tmp/data");
        assert!(Arc::ptr_eq(&r2, &tmp_fs));
        assert_eq!(p2, "data");

        let (r3, p3) = resolve_fs("/etc/conf");
        assert!(Arc::ptr_eq(&r3, &root_fs));
        assert_eq!(p3, "etc/conf");

        unmount("/tmp").unwrap();
        unmount("/disk").unwrap();
        unmount("/").unwrap();
    }
}
