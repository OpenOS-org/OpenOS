//! Simple in-memory filesystem (ramfs).
//!
//! Provides basic file operations: create, read, write, delete.
//! Files are stored in a static array with a simple table structure.
//! Implements the VFS `FileSystem` trait for use by the VFS layer.
//!
//! ## Design
//!
//! - Storage: 64 KiB static array
//! - Max files: 32
//! - Max filename: 63 bytes (null-terminated)
//! - Max file size: 2048 bytes
//! - No directories (flat namespace)
//! - Inode numbers: root = 0, files = index + 1

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use spin::Mutex;

use super::vfs::{DirEntry, FileSystem, FsError, InodeMeta, OpenFlags};
use crate::ipc::pipe;

/// Maximum number of files.
const MAX_FILES: usize = 32;

/// Maximum filename length (including null terminator).
const MAX_NAME_LEN: usize = 64;

/// Maximum file size in bytes.
const MAX_FILE_SIZE: usize = 2048;

/// Inode number of the root directory.
const ROOT_INO: u64 = 0;

/// Inode number offset for regular files (file index + 1).
const FILE_INO_OFFSET: u64 = 1;

/// A file entry in the ramfs.
struct FileEntry {
    /// Filename (null-terminated).
    name: [u8; MAX_NAME_LEN],
    /// File data.
    data: Vec<u8>,
    /// Whether this slot is in use.
    in_use: bool,
    /// Symlink target path, if this entry is a symbolic link.
    symlink_target: Option<String>,
    /// Whether this entry is a directory.
    is_dir: bool,
    /// Whether this entry is a named pipe (FIFO).
    is_fifo: bool,
    /// Child entries (name, slot index) — only used when `is_dir` is true.
    children: Vec<([u8; MAX_NAME_LEN], usize)>,
    /// Shared pipe buffer for FIFO nodes.
    #[allow(dead_code)]
    fifo_buffer: Option<Arc<Mutex<pipe::PipeBuffer>>>,
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
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
                },
                FileEntry {
                    name: [0; MAX_NAME_LEN],
                    data: Vec::new(),
                    in_use: false,
                    symlink_target: None,
                    is_dir: false,
                    is_fifo: false,
                    children: Vec::new(),
                    fifo_buffer: None,
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

    /// Get the name of a file entry as a `&str`.
    fn entry_name(&self, idx: usize) -> &str {
        let entry = &self.files[idx];
        let name_len = entry
            .name
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(MAX_NAME_LEN);
        core::str::from_utf8(&entry.name[..name_len]).unwrap_or("")
    }

    /// Reset a slot to its default (unused) state.
    ///
    /// Clears all fields so the slot can be safely reused by a new entry without
    /// stale state leaking from the previous lifetime.
    fn clear_slot(&mut self, idx: usize) {
        if idx >= MAX_FILES {
            return;
        }
        let entry = &mut self.files[idx];
        entry.name = [0; MAX_NAME_LEN];
        entry.data.clear();
        entry.in_use = false;
        entry.symlink_target = None;
        entry.is_dir = false;
        entry.is_fifo = false;
        entry.children.clear();
        entry.fifo_buffer = None;
    }

    /// Initialize a slot for a new entry (file or directory).
    ///
    /// Sets the name, marks the slot in use, and resets all type-specific fields
    /// to defaults. The caller must set `is_dir`/`is_fifo` etc. as needed.
    fn init_slot(
        &mut self,
        slot: usize,
        name: &str,
    ) {
        debug_assert!(slot < MAX_FILES, "slot out of range");
        debug_assert!(!self.files[slot].in_use, "slot already in use");
        let entry = &mut self.files[slot];
        entry.name = [0; MAX_NAME_LEN];
        let name_bytes = name.as_bytes();
        entry.name[..name_bytes.len()].copy_from_slice(name_bytes);
        entry.data = Vec::new();
        entry.in_use = true;
        entry.symlink_target = None;
        entry.is_dir = false;
        entry.is_fifo = false;
        entry.children = Vec::new();
        entry.fifo_buffer = None;
    }

    /// Find a child by name within a directory entry.
    fn find_child(&self, dir_idx: usize, name: &str) -> Option<usize> {
        if !self.files[dir_idx].is_dir {
            return None;
        }
        for (child_name, child_idx) in &self.files[dir_idx].children {
            let cn_len = child_name
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(MAX_NAME_LEN);
            if let Ok(cn) = core::str::from_utf8(&child_name[..cn_len]) {
                if cn == name {
                    return Some(*child_idx);
                }
            }
        }
        None
    }
}

/// Errors that can occur in ramfs operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RamFsError {
    /// File not found.
    NotFound,
    /// File already exists.
    AlreadyExists,
    /// No space left for new files.
    NoSpace,
    /// File is too large.
    FileTooLarge,
    /// Invalid file name.
    InvalidName,
}

impl From<RamFsError> for FsError {
    fn from(e: RamFsError) -> Self {
        match e {
            RamFsError::NotFound => Self::NotFound,
            RamFsError::AlreadyExists => Self::AlreadyExists,
            RamFsError::NoSpace => Self::NoSpace,
            RamFsError::FileTooLarge => Self::FileTooLarge,
            RamFsError::InvalidName => Self::InvalidName,
        }
    }
}

/// `RamFS` implementation of the VFS `FileSystem` trait.
///
/// Provides `open`, `close`, `read`, `write`, `stat`, `readdir`,
/// `create`, and `unlink` operations on the in-memory filesystem.
pub struct RamFsVfs;

impl FileSystem for RamFsVfs {
    fn open(&self, path: &str, flags: OpenFlags) -> Result<u64, FsError> {
        let mut fs = RAMFS.lock();

        // Strip leading '/' for flat namespace.
        let name = path.trim_start_matches('/');

        // Root directory — always accessible for reading.
        if name.is_empty() {
            Self::ensure_root_dir(&mut fs);
            return Ok(ROOT_INO);
        }

        if let Some(idx) = fs.find_file(name) {
            // File exists.
            if flags.contains(OpenFlags::TRUNCATE) {
                fs.files[idx].data.clear();
            }
            // Return the inode number as the file descriptor.
            return Ok(idx as u64 + FILE_INO_OFFSET);
        }

        // File not found -- create if CREATE flag is set.
        if flags.contains(OpenFlags::CREATE) {
            let slot = fs.find_free_slot().ok_or(FsError::NoSpace)?;
            fs.init_slot(slot, name);

            // Register in root directory's children list if it exists.
            let child_name = fs.files[slot].name;
            if let Some(root_idx) = Self::get_root_dir_idx(&fs) {
                fs.files[root_idx].children.push((child_name, slot));
            }

            return Ok(slot as u64 + FILE_INO_OFFSET);
        }

        Err(FsError::NotFound)
    }

    fn close(&self, _ino: u64) -> Result<(), FsError> {
        // RamFS has no open-file state to clean up.
        Ok(())
    }

    fn read(&self, ino: u64, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        let fs = RAMFS.lock();

        if ino == ROOT_INO {
            return Err(FsError::NotSupported);
        }

        let idx = (ino - FILE_INO_OFFSET) as usize;
        if idx >= MAX_FILES || !fs.files[idx].in_use {
            return Err(FsError::NotFound);
        }

        let data = &fs.files[idx].data;
        let off = offset as usize;

        if off >= data.len() {
            return Ok(0); // EOF
        }

        let available = data.len() - off;
        let to_read = buf.len().min(available);
        buf[..to_read].copy_from_slice(&data[off..off + to_read]);
        Ok(to_read)
    }

    fn write(&self, ino: u64, offset: u64, data: &[u8]) -> Result<usize, FsError> {
        let mut fs = RAMFS.lock();

        if ino == ROOT_INO {
            return Err(FsError::NotSupported);
        }

        let idx = (ino - FILE_INO_OFFSET) as usize;
        if idx >= MAX_FILES || !fs.files[idx].in_use {
            return Err(FsError::NotFound);
        }

        let end = offset as usize + data.len();
        if end > MAX_FILE_SIZE {
            return Err(FsError::FileTooLarge);
        }

        let file = &mut fs.files[idx];
        // Extend file if writing past current end.
        if end > file.data.len() {
            file.data.resize(end, 0);
        }
        file.data[offset as usize..end].copy_from_slice(data);
        Ok(data.len())
    }

    fn stat(&self, ino: u64) -> Result<InodeMeta, FsError> {
        let fs = RAMFS.lock();

        if ino == ROOT_INO {
            // Root directory.
            let root_children =
                Self::get_root_dir_idx(&fs).map_or(0, |idx| fs.files[idx].children.len());
            return Ok(InodeMeta {
                ino: ROOT_INO,
                is_dir: true,
                is_fifo: false,
                is_symlink: false,
                size: root_children as u64,
                nlink: 1,
            });
        }

        let idx = (ino - FILE_INO_OFFSET) as usize;
        if idx >= MAX_FILES || !fs.files[idx].in_use {
            return Err(FsError::NotFound);
        }

        let entry = &fs.files[idx];
        let size = if entry.is_dir {
            entry.children.len() as u64
        } else {
            entry.data.len() as u64
        };

        Ok(InodeMeta {
            ino,
            is_dir: entry.is_dir,
            is_symlink: entry.symlink_target.is_some(),
            is_fifo: entry.is_fifo,
            size,
            nlink: 1,
        })
    }

    fn readdir(&self, dir_ino: u64) -> Result<Vec<DirEntry>, FsError> {
        let fs = RAMFS.lock();
        let mut entries = Vec::new();

        // Determine the parent inode for "..".
        let parent_ino = dir_ino;

        if dir_ino == ROOT_INO {
            // Root directory: list root-level entries.
            entries.push(DirEntry {
                name: String::from("."),
                ino: ROOT_INO,
                is_dir: true,
            });
            entries.push(DirEntry {
                name: String::from(".."),
                ino: ROOT_INO,
                is_dir: true,
            });

            // List children tracked in the root dir entry (if it exists).
            if let Some(root_idx) = Self::get_root_dir_idx(&fs) {
                for (child_name, child_idx) in &fs.files[root_idx].children {
                    let cn_len = child_name
                        .iter()
                        .position(|&b| b == 0)
                        .unwrap_or(MAX_NAME_LEN);
                    if let Ok(cn) = core::str::from_utf8(&child_name[..cn_len]) {
                        let child = &fs.files[*child_idx];
                        if child.in_use {
                            entries.push(DirEntry {
                                name: String::from(cn),
                                ino: *child_idx as u64 + FILE_INO_OFFSET,
                                is_dir: child.is_dir,
                            });
                        }
                    }
                }
            } else {
                // No root dir entry yet — list all top-level in-use entries
                // that are NOT directories (backward compat with flat mode).
                for (i, f) in fs.files.iter().enumerate() {
                    if f.in_use && !f.is_dir {
                        let name_len = f.name.iter().position(|&b| b == 0).unwrap_or(MAX_NAME_LEN);
                        if let Ok(name) = core::str::from_utf8(&f.name[..name_len]) {
                            entries.push(DirEntry {
                                name: String::from(name),
                                ino: i as u64 + FILE_INO_OFFSET,
                                is_dir: false,
                            });
                        }
                    }
                }
            }
        } else {
            // Subdirectory: list children of this directory.
            let idx = (dir_ino - FILE_INO_OFFSET) as usize;
            if idx >= MAX_FILES || !fs.files[idx].in_use || !fs.files[idx].is_dir {
                return Err(FsError::NotFound);
            }

            entries.push(DirEntry {
                name: String::from("."),
                ino: dir_ino,
                is_dir: true,
            });
            entries.push(DirEntry {
                name: String::from(".."),
                ino: parent_ino,
                is_dir: true,
            });

            for (child_name, child_idx) in &fs.files[idx].children {
                let cn_len = child_name
                    .iter()
                    .position(|&b| b == 0)
                    .unwrap_or(MAX_NAME_LEN);
                if let Ok(cn) = core::str::from_utf8(&child_name[..cn_len]) {
                    let child = &fs.files[*child_idx];
                    if child.in_use {
                        entries.push(DirEntry {
                            name: String::from(cn),
                            ino: *child_idx as u64 + FILE_INO_OFFSET,
                            is_dir: child.is_dir,
                        });
                    }
                }
            }
        }

        Ok(entries)
    }

    fn create(&self, parent_ino: u64, name: &str) -> Result<u64, FsError> {
        if name.is_empty() || name.len() >= MAX_NAME_LEN {
            return Err(FsError::InvalidName);
        }

        let mut fs = RAMFS.lock();

        // Resolve parent directory.
        let parent_idx = if parent_ino == ROOT_INO {
            Self::ensure_root_dir(&mut fs)
        } else {
            let idx = (parent_ino - FILE_INO_OFFSET) as usize;
            if idx >= MAX_FILES || !fs.files[idx].in_use || !fs.files[idx].is_dir {
                return Err(FsError::NotFound);
            }
            idx
        };

        // Check for duplicate name in parent.
        if fs.find_child(parent_idx, name).is_some() {
            return Err(FsError::AlreadyExists);
        }

        let slot = fs.find_free_slot().ok_or(FsError::NoSpace)?;
        fs.init_slot(slot, name);

        // Register in parent directory's children list.
        let child_name = fs.files[slot].name;
        fs.files[parent_idx].children.push((child_name, slot));

        Ok(slot as u64 + FILE_INO_OFFSET)
    }

    fn unlink(&self, parent_ino: u64, name: &str) -> Result<(), FsError> {
        let mut fs = RAMFS.lock();

        // Resolve parent directory.
        let parent_idx = if parent_ino == ROOT_INO {
            if let Some(idx) = Self::get_root_dir_idx(&fs) {
                idx
            } else {
                // No root dir entry yet — fall back to flat mode lookup.
                let idx = fs.find_file(name).ok_or(FsError::NotFound)?;
                fs.files[idx].in_use = false;
                fs.files[idx].data.clear();
                fs.files[idx].name = [0; MAX_NAME_LEN];
                fs.files[idx].symlink_target = None;
                return Ok(());
            }
        } else {
            let idx = (parent_ino - FILE_INO_OFFSET) as usize;
            if idx >= MAX_FILES || !fs.files[idx].in_use || !fs.files[idx].is_dir {
                return Err(FsError::NotFound);
            }
            idx
        };

        // Find the child in the parent.
        let child_idx = fs.find_child(parent_idx, name).ok_or(FsError::NotFound)?;

        // Remove from parent's children list.
        fs.files[parent_idx]
            .children
            .retain(|(_, ci)| *ci != child_idx);

        fs.clear_slot(child_idx);
        Ok(())
    }

    fn symlink(&self, parent_ino: u64, name: &str, target: &str) -> Result<u64, FsError> {
        if name.is_empty() || name.len() >= MAX_NAME_LEN {
            return Err(FsError::InvalidName);
        }

        let mut fs = RAMFS.lock();

        // Resolve parent directory.
        let parent_idx = if parent_ino == ROOT_INO {
            // Check for duplicates via both flat mode and root dir.
            if fs.find_file(name).is_some() {
                return Err(FsError::AlreadyExists);
            }
            Self::ensure_root_dir(&mut fs)
        } else {
            let idx = (parent_ino - FILE_INO_OFFSET) as usize;
            if idx >= MAX_FILES || !fs.files[idx].in_use || !fs.files[idx].is_dir {
                return Err(FsError::NotFound);
            }
            if fs.find_child(idx, name).is_some() {
                return Err(FsError::AlreadyExists);
            }
            idx
        };

        let slot = fs.find_free_slot().ok_or(FsError::NoSpace)?;
        fs.init_slot(slot, name);
        fs.files[slot].symlink_target = Some(String::from(target));

        // Register in parent directory's children list.
        let child_name = fs.files[slot].name;
        fs.files[parent_idx].children.push((child_name, slot));

        Ok(slot as u64 + FILE_INO_OFFSET)
    }

    fn readlink(&self, ino: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        let fs = RAMFS.lock();

        if ino == ROOT_INO {
            return Err(FsError::NotSupported);
        }

        let idx = (ino - FILE_INO_OFFSET) as usize;
        if idx >= MAX_FILES || !fs.files[idx].in_use {
            return Err(FsError::NotFound);
        }

        let target = fs.files[idx]
            .symlink_target
            .as_ref()
            .ok_or(FsError::NotSupported)?;
        let target_bytes = target.as_bytes();
        let to_copy = buf.len().min(target_bytes.len());
        buf[..to_copy].copy_from_slice(&target_bytes[..to_copy]);
        Ok(to_copy)
    }

    fn mkdir(&self, parent_ino: u64, name: &str) -> Result<u64, FsError> {
        if name.is_empty() || name.len() >= MAX_NAME_LEN {
            return Err(FsError::InvalidName);
        }

        let mut fs = RAMFS.lock();

        // Resolve the parent directory index.
        let parent_idx = if parent_ino == ROOT_INO {
            Self::ensure_root_dir(&mut fs)
        } else {
            let idx = (parent_ino - FILE_INO_OFFSET) as usize;
            if idx >= MAX_FILES || !fs.files[idx].in_use || !fs.files[idx].is_dir {
                return Err(FsError::NotADirectory);
            }
            idx
        };

        // Check for duplicate name in parent.
        if fs.find_child(parent_idx, name).is_some() {
            return Err(FsError::AlreadyExists);
        }

        // Allocate a new slot for the directory.
        let slot = fs.find_free_slot().ok_or(FsError::NoSpace)?;
        fs.init_slot(slot, name);
        fs.files[slot].is_dir = true;

        // Record the child in the parent's children list.
        let child_name = fs.files[slot].name;
        fs.files[parent_idx].children.push((child_name, slot));

        let child_ino = slot as u64 + FILE_INO_OFFSET;
        crate::serial_println!("[ramfs] mkdir: '{}' -> ino {}", name, child_ino);
        Ok(child_ino)
    }

    fn rmdir(&self, parent_ino: u64, name: &str) -> Result<(), FsError> {
        let mut fs = RAMFS.lock();

        // Resolve the parent directory index.
        let parent_idx = if parent_ino == ROOT_INO {
            match Self::get_root_dir_idx(&fs) {
                Some(idx) => idx,
                None => return Err(FsError::NotFound),
            }
        } else {
            let idx = (parent_ino - FILE_INO_OFFSET) as usize;
            if idx >= MAX_FILES || !fs.files[idx].in_use || !fs.files[idx].is_dir {
                return Err(FsError::NotADirectory);
            }
            idx
        };

        // Find the child in the parent.
        let child_idx = fs.find_child(parent_idx, name).ok_or(FsError::NotFound)?;

        // Verify it is a directory.
        if !fs.files[child_idx].is_dir {
            return Err(FsError::NotADirectory);
        }

        // Verify it is empty (no children other than . and ..).
        if !fs.files[child_idx].children.is_empty() {
            return Err(FsError::IoError);
        }

        // Remove the child from the parent's children list.
        fs.files[parent_idx]
            .children
            .retain(|(_, idx)| *idx != child_idx);

        // Free the slot.
        fs.clear_slot(child_idx);

        crate::serial_println!("[ramfs] rmdir: '{}'", name);
        Ok(())
    }
}

impl RamFsVfs {
    /// Find the root directory slot index, if it exists.
    fn get_root_dir_idx(fs: &RamFs) -> Option<usize> {
        for (i, f) in fs.files.iter().enumerate() {
            if f.in_use && f.is_dir {
                let name_len = f.name.iter().position(|&b| b == 0).unwrap_or(MAX_NAME_LEN);
                if let Ok(n) = core::str::from_utf8(&f.name[..name_len]) {
                    if n == "." {
                        return Some(i);
                    }
                }
            }
        }
        None
    }

    /// Ensure the root directory entry exists. Returns its slot index.
    fn ensure_root_dir(fs: &mut RamFs) -> usize {
        if let Some(idx) = Self::get_root_dir_idx(fs) {
            return idx;
        }
        // Create a root directory entry named "." (virtual root).
        let slot = fs.find_free_slot().expect("no space for root dir");
        let entry = &mut fs.files[slot];
        entry.name = [0; MAX_NAME_LEN];
        entry.name[0] = b'.';
        entry.data = Vec::new();
        entry.in_use = true;
        entry.is_dir = true;
        entry.is_fifo = false;
        entry.children = Vec::new();
        entry.symlink_target = None;
        entry.fifo_buffer = None;
        slot
    }
}

/// Create a new file with the given name and data.
///
/// This is the legacy API kept for backward compatibility.
/// New code should use `RamFsVfs` via the `FileSystem` trait.
///
/// # Errors
///
/// Returns `RamFsError` if the name is invalid, file already exists,
/// no space is available, or the file is too large.
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
///
/// This is the legacy API kept for backward compatibility.
///
/// # Errors
///
/// Returns `RamFsError::NotFound` if the file does not exist.
pub fn read_file(name: &str) -> Result<Vec<u8>, RamFsError> {
    let fs = RAMFS.lock();
    let idx = fs.find_file(name).ok_or(RamFsError::NotFound)?;
    Ok(fs.files[idx].data.clone())
}

/// Write data to an existing file (replaces contents).
///
/// This is the legacy API kept for backward compatibility.
///
/// # Errors
///
/// Returns `RamFsError::NotFound` if the file does not exist.
/// Returns `RamFsError::FileTooLarge` if the data exceeds the maximum file size.
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

/// Write data at a specific offset in an existing file.
///
/// If the offset is beyond the current file size, the gap is filled with zeros.
/// Returns the number of bytes written.
///
/// # Errors
///
/// Returns `RamFsError::NotFound` if the file does not exist.
/// Returns `RamFsError::FileTooLarge` if the write would exceed the maximum file size.
pub fn write_file_at(name: &str, offset: usize, data: &[u8]) -> Result<usize, RamFsError> {
    if data.is_empty() {
        return Ok(0);
    }

    let end = offset
        .checked_add(data.len())
        .ok_or(RamFsError::FileTooLarge)?;
    if end > MAX_FILE_SIZE {
        return Err(RamFsError::FileTooLarge);
    }

    let mut fs = RAMFS.lock();
    let idx = fs.find_file(name).ok_or(RamFsError::NotFound)?;
    let entry = &mut fs.files[idx];

    // Extend with zeros if writing past the current end.
    if offset > entry.data.len() {
        entry.data.resize(offset, 0);
    }
    // Ensure the buffer is large enough.
    if end > entry.data.len() {
        entry.data.resize(end, 0);
    }

    entry.data[offset..end].copy_from_slice(data);
    Ok(data.len())
}

/// Get the size of a file.
///
/// # Errors
///
/// Returns `RamFsError::NotFound` if the file does not exist.
pub fn file_size(name: &str) -> Result<usize, RamFsError> {
    let fs = RAMFS.lock();
    let idx = fs.find_file(name).ok_or(RamFsError::NotFound)?;
    Ok(fs.files[idx].data.len())
}

/// Delete a file.
///
/// This is the legacy API kept for backward compatibility.
///
/// # Errors
///
/// Returns `RamFsError::NotFound` if the file does not exist.
pub fn delete_file(name: &str) -> Result<(), RamFsError> {
    let mut fs = RAMFS.lock();
    let idx = fs.find_file(name).ok_or(RamFsError::NotFound)?;
    fs.files[idx].in_use = false;
    fs.files[idx].data.clear();
    fs.files[idx].name = [0; MAX_NAME_LEN];
    Ok(())
}

/// List all files in the ramfs.
///
/// This is the legacy API kept for backward compatibility.
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
        MAX_FILES * MAX_FILE_SIZE
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_read() {
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

    #[test]
    fn test_write_file_at() {
        create_file("w_at.txt", b"old").unwrap();
        let written = write_file_at("w_at.txt", 4, b"new").unwrap();
        assert_eq!(written, 3);
        let data = read_file("w_at.txt").unwrap();
        assert_eq!(data, b"old\x00new");
        delete_file("w_at.txt").unwrap();
    }

    #[test]
    fn test_file_size_fn() {
        create_file("sz.txt", b"hello").unwrap();
        assert_eq!(file_size("sz.txt").unwrap(), 5);
        delete_file("sz.txt").unwrap();
    }

    // --- VFS trait tests ---

    #[test]
    fn test_vfs_create_and_open() {
        let vfs = RamFsVfs;
        let ino = vfs.create(ROOT_INO, "vfs_test.txt").unwrap();
        assert!(ino > 0);

        let fd = vfs.open("vfs_test.txt", OpenFlags::READ).unwrap();
        assert!(fd > 0);

        vfs.close(fd).unwrap();
        vfs.unlink(ROOT_INO, "vfs_test.txt").unwrap();
    }

    #[test]
    fn test_vfs_open_with_create() {
        let vfs = RamFsVfs;
        let fd = vfs
            .open("auto_create.txt", OpenFlags::CREATE | OpenFlags::READ_WRITE)
            .unwrap();
        assert!(fd > 0);
        vfs.close(fd).unwrap();
        vfs.unlink(ROOT_INO, "auto_create.txt").unwrap();
    }

    #[test]
    fn test_vfs_read_write() {
        let vfs = RamFsVfs;
        let ino = vfs
            .open("rw_test.txt", OpenFlags::CREATE | OpenFlags::READ_WRITE)
            .unwrap();

        let written = vfs.write(ino, 0, b"hello world").unwrap();
        assert_eq!(written, 11);

        let mut buf = [0u8; 64];
        let read = vfs.read(ino, 0, &mut buf).unwrap();
        assert_eq!(read, 11);
        assert_eq!(&buf[..read], b"hello world");

        vfs.close(ino).unwrap();
        vfs.unlink(ROOT_INO, "rw_test.txt").unwrap();
    }

    #[test]
    fn test_vfs_stat() {
        let vfs = RamFsVfs;
        let ino = vfs.create(ROOT_INO, "stat_test.txt").unwrap();

        let meta = vfs.stat(ino).unwrap();
        assert!(!meta.is_dir);
        assert_eq!(meta.size, 0);

        let root_meta = vfs.stat(ROOT_INO).unwrap();
        assert!(root_meta.is_dir);

        vfs.unlink(ROOT_INO, "stat_test.txt").unwrap();
    }

    #[test]
    fn test_vfs_readdir() {
        let vfs = RamFsVfs;
        vfs.create(ROOT_INO, "rd_a.txt").unwrap();
        vfs.create(ROOT_INO, "rd_b.txt").unwrap();

        let entries = vfs.readdir(ROOT_INO).unwrap();
        // Should have at least "." and ".." plus our two files.
        assert!(entries.len() >= 4);
        assert!(entries.iter().any(|e| e.name == "."));
        assert!(entries.iter().any(|e| e.name == ".."));
        assert!(entries.iter().any(|e| e.name == "rd_a.txt"));
        assert!(entries.iter().any(|e| e.name == "rd_b.txt"));

        vfs.unlink(ROOT_INO, "rd_a.txt").unwrap();
        vfs.unlink(ROOT_INO, "rd_b.txt").unwrap();
    }

    #[test]
    fn test_vfs_unlink() {
        let vfs = RamFsVfs;
        vfs.create(ROOT_INO, "unlink_test.txt").unwrap();
        vfs.unlink(ROOT_INO, "unlink_test.txt").unwrap();

        // Should fail to open after unlink.
        assert_eq!(
            vfs.open("unlink_test.txt", OpenFlags::READ),
            Err(FsError::NotFound)
        );
    }

    #[test]
    fn test_vfs_close_is_noop() {
        let vfs = RamFsVfs;
        // RamFS close always succeeds (no open-file state).
        assert!(vfs.close(9999).is_ok());
    }

    #[test]
    fn test_vfs_read_bad_ino() {
        let vfs = RamFsVfs;
        let mut buf = [0u8; 16];
        assert_eq!(vfs.read(9999, 0, &mut buf), Err(FsError::NotFound));
    }

    #[test]
    fn test_vfs_read_at_eof() {
        let vfs = RamFsVfs;
        let ino = vfs
            .open("eof_test.txt", OpenFlags::CREATE | OpenFlags::READ_WRITE)
            .unwrap();
        vfs.write(ino, 0, b"abc").unwrap();

        let mut buf = [0u8; 16];
        let read = vfs.read(ino, 100, &mut buf).unwrap();
        assert_eq!(read, 0);

        vfs.close(ino).unwrap();
        vfs.unlink(ROOT_INO, "eof_test.txt").unwrap();
    }

    #[test]
    fn test_vfs_create_duplicate() {
        let vfs = RamFsVfs;
        vfs.create(ROOT_INO, "dup_vfs.txt").unwrap();
        assert_eq!(
            vfs.create(ROOT_INO, "dup_vfs.txt"),
            Err(FsError::AlreadyExists)
        );
        vfs.unlink(ROOT_INO, "dup_vfs.txt").unwrap();
    }

    #[test]
    fn test_vfs_readdir_non_root() {
        let vfs = RamFsVfs;
        assert_eq!(vfs.readdir(999), Err(FsError::NotFound));
    }

    #[test]
    fn test_vfs_open_truncate() {
        let vfs = RamFsVfs;
        let ino = vfs
            .open("trunc.txt", OpenFlags::CREATE | OpenFlags::READ_WRITE)
            .unwrap();
        vfs.write(ino, 0, b"long data here").unwrap();
        vfs.close(ino).unwrap();

        let ino2 = vfs
            .open("trunc.txt", OpenFlags::READ_WRITE | OpenFlags::TRUNCATE)
            .unwrap();
        let mut buf = [0u8; 64];
        let read = vfs.read(ino2, 0, &mut buf).unwrap();
        assert_eq!(read, 0);

        vfs.close(ino2).unwrap();
        vfs.unlink(ROOT_INO, "trunc.txt").unwrap();
    }

    // --- Symlink tests ---

    #[test]
    fn test_vfs_symlink_and_readlink() {
        let vfs = RamFsVfs;
        let ino = vfs.symlink(ROOT_INO, "link1", "/some/target").unwrap();
        assert!(ino > 0);

        let mut buf = [0u8; 64];
        let n = vfs.readlink(ino, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"/some/target");

        vfs.unlink(ROOT_INO, "link1").unwrap();
    }

    #[test]
    fn test_vfs_symlink_duplicate() {
        let vfs = RamFsVfs;
        vfs.symlink(ROOT_INO, "dup_link", "/a").unwrap();
        assert_eq!(
            vfs.symlink(ROOT_INO, "dup_link", "/b"),
            Err(FsError::AlreadyExists)
        );
        vfs.unlink(ROOT_INO, "dup_link").unwrap();
    }

    #[test]
    fn test_vfs_symlink_invalid_name() {
        let vfs = RamFsVfs;
        assert_eq!(
            vfs.symlink(ROOT_INO, "", "/target"),
            Err(FsError::InvalidName)
        );
    }

    #[test]
    fn test_vfs_symlink_non_root() {
        let vfs = RamFsVfs;
        assert_eq!(vfs.symlink(999, "link", "/target"), Err(FsError::NotFound));
    }

    #[test]
    fn test_vfs_readlink_not_symlink() {
        let vfs = RamFsVfs;
        let ino = vfs.create(ROOT_INO, "not_a_link.txt").unwrap();
        let mut buf = [0u8; 16];
        assert_eq!(vfs.readlink(ino, &mut buf), Err(FsError::NotSupported));
        vfs.unlink(ROOT_INO, "not_a_link.txt").unwrap();
    }

    #[test]
    fn test_vfs_readlink_bad_ino() {
        let vfs = RamFsVfs;
        let mut buf = [0u8; 16];
        assert_eq!(vfs.readlink(9999, &mut buf), Err(FsError::NotFound));
    }

    #[test]
    fn test_vfs_symlink_stat() {
        let vfs = RamFsVfs;
        let ino = vfs.symlink(ROOT_INO, "stat_link", "/foo").unwrap();

        let meta = vfs.stat(ino).unwrap();
        assert!(meta.is_symlink);
        assert!(!meta.is_dir);

        vfs.unlink(ROOT_INO, "stat_link").unwrap();
    }

    #[test]
    fn test_vfs_symlink_readlink_truncated() {
        let vfs = RamFsVfs;
        let ino = vfs
            .symlink(ROOT_INO, "short_buf", "/long/target/path")
            .unwrap();

        let mut buf = [0u8; 4];
        let n = vfs.readlink(ino, &mut buf).unwrap();
        assert_eq!(n, 4);
        assert_eq!(&buf[..n], b"/lon");

        vfs.unlink(ROOT_INO, "short_buf").unwrap();
    }

    // --- mkdir / rmdir tests ---

    #[test]
    fn test_vfs_mkdir_and_stat() {
        let vfs = RamFsVfs;
        let dir_ino = vfs.mkdir(ROOT_INO, "testdir").unwrap();
        assert!(dir_ino > 0);

        let meta = vfs.stat(dir_ino).unwrap();
        assert!(meta.is_dir);
        assert_eq!(meta.size, 0); // empty directory

        vfs.rmdir(ROOT_INO, "testdir").unwrap();
    }

    #[test]
    fn test_vfs_mkdir_duplicate() {
        let vfs = RamFsVfs;
        vfs.mkdir(ROOT_INO, "dupdir").unwrap();
        assert_eq!(vfs.mkdir(ROOT_INO, "dupdir"), Err(FsError::AlreadyExists));
        vfs.rmdir(ROOT_INO, "dupdir").unwrap();
    }

    #[test]
    fn test_vfs_mkdir_invalid_name() {
        let vfs = RamFsVfs;
        assert_eq!(vfs.mkdir(ROOT_INO, ""), Err(FsError::InvalidName));
    }

    #[test]
    fn test_vfs_rmdir_not_found() {
        let vfs = RamFsVfs;
        assert_eq!(vfs.rmdir(ROOT_INO, "noexist"), Err(FsError::NotFound));
    }

    #[test]
    fn test_vfs_rmdir_not_empty() {
        let vfs = RamFsVfs;
        let dir_ino = vfs.mkdir(ROOT_INO, "notempty").unwrap();
        // Create a file inside the directory.
        vfs.create(dir_ino, "child.txt").unwrap();

        // rmdir should fail because directory is not empty.
        assert_eq!(vfs.rmdir(ROOT_INO, "notempty"), Err(FsError::IoError));

        // Clean up: remove the child, then rmdir.
        vfs.unlink(dir_ino, "child.txt").unwrap();
        vfs.rmdir(ROOT_INO, "notempty").unwrap();
    }

    #[test]
    fn test_vfs_mkdir_readdir() {
        let vfs = RamFsVfs;
        let dir_ino = vfs.mkdir(ROOT_INO, "mydir").unwrap();

        // Create a file inside the directory.
        vfs.create(dir_ino, "file.txt").unwrap();

        // readdir on the directory should show ".", "..", and "file.txt".
        let entries = vfs.readdir(dir_ino).unwrap();
        assert!(entries.iter().any(|e| e.name == "."));
        assert!(entries.iter().any(|e| e.name == ".."));
        assert!(entries.iter().any(|e| e.name == "file.txt"));

        // Clean up.
        vfs.unlink(dir_ino, "file.txt").unwrap();
        vfs.rmdir(ROOT_INO, "mydir").unwrap();
    }

    #[test]
    fn test_vfs_mkdir_nested() {
        let vfs = RamFsVfs;
        let dir1 = vfs.mkdir(ROOT_INO, "parent").unwrap();
        let dir2 = vfs.mkdir(dir1, "child").unwrap();

        let meta = vfs.stat(dir2).unwrap();
        assert!(meta.is_dir);

        // readdir on parent should contain "child".
        let entries = vfs.readdir(dir1).unwrap();
        assert!(entries.iter().any(|e| e.name == "child" && e.is_dir));

        // Clean up.
        vfs.rmdir(dir1, "child").unwrap();
        vfs.rmdir(ROOT_INO, "parent").unwrap();
    }

    #[test]
    fn test_vfs_mkdir_in_readdir_root() {
        let vfs = RamFsVfs;
        vfs.mkdir(ROOT_INO, "visible").unwrap();

        let entries = vfs.readdir(ROOT_INO).unwrap();
        let dir_entry = entries.iter().find(|e| e.name == "visible");
        assert!(dir_entry.is_some());
        assert!(dir_entry.unwrap().is_dir);

        vfs.rmdir(ROOT_INO, "visible").unwrap();
    }

    #[test]
    fn test_vfs_stat_dir_is_dir() {
        let vfs = RamFsVfs;
        let dir_ino = vfs.mkdir(ROOT_INO, "checkdir").unwrap();

        let meta = vfs.stat(dir_ino).unwrap();
        assert!(meta.is_dir);

        // A regular file should not be a directory.
        let file_ino = vfs.create(ROOT_INO, "checkfile.txt").unwrap();
        let file_meta = vfs.stat(file_ino).unwrap();
        assert!(!file_meta.is_dir);

        vfs.unlink(ROOT_INO, "checkfile.txt").unwrap();
        vfs.rmdir(ROOT_INO, "checkdir").unwrap();
    }

    // --- Comprehensive directory tests (requested) ---

    #[test]
    fn test_mkdir_creates_directory() {
        // Create a directory and verify readdir shows it.
        let vfs = RamFsVfs;
        let dir_ino = vfs.mkdir(ROOT_INO, "newdir").unwrap();
        assert!(dir_ino > 0);

        let entries = vfs.readdir(ROOT_INO).unwrap();
        let found = entries.iter().find(|e| e.name == "newdir");
        assert!(found.is_some());
        assert!(found.unwrap().is_dir);

        vfs.rmdir(ROOT_INO, "newdir").unwrap();
    }

    #[test]
    fn test_mkdir_nested_directories() {
        // Create parent/child dirs and verify the hierarchy.
        let vfs = RamFsVfs;
        let parent_ino = vfs.mkdir(ROOT_INO, "parent").unwrap();
        let child_ino = vfs.mkdir(parent_ino, "child").unwrap();
        assert!(child_ino > 0);

        // Verify child appears in parent's readdir.
        let parent_entries = vfs.readdir(parent_ino).unwrap();
        assert!(parent_entries.iter().any(|e| e.name == "child" && e.is_dir));

        // Verify parent appears in root readdir.
        let root_entries = vfs.readdir(ROOT_INO).unwrap();
        assert!(root_entries.iter().any(|e| e.name == "parent" && e.is_dir));

        // Clean up: remove child first, then parent.
        vfs.rmdir(parent_ino, "child").unwrap();
        vfs.rmdir(ROOT_INO, "parent").unwrap();
    }

    #[test]
    fn test_rmdir_empty_directory() {
        // Create an empty directory and remove it successfully.
        let vfs = RamFsVfs;
        vfs.mkdir(ROOT_INO, "emptydir").unwrap();

        // Verify it exists.
        let entries = vfs.readdir(ROOT_INO).unwrap();
        assert!(entries.iter().any(|e| e.name == "emptydir"));

        // Remove it.
        vfs.rmdir(ROOT_INO, "emptydir").unwrap();

        // Verify it is gone.
        let entries = vfs.readdir(ROOT_INO).unwrap();
        assert!(!entries.iter().any(|e| e.name == "emptydir"));
    }

    #[test]
    fn test_rmdir_non_empty_directory_fails() {
        // Create a directory with a file inside, verify rmdir fails.
        let vfs = RamFsVfs;
        let dir_ino = vfs.mkdir(ROOT_INO, "nonempty").unwrap();
        vfs.create(dir_ino, "child.txt").unwrap();

        // rmdir should fail because directory is not empty.
        assert_eq!(vfs.rmdir(ROOT_INO, "nonempty"), Err(FsError::IoError));

        // Directory should still exist.
        let entries = vfs.readdir(ROOT_INO).unwrap();
        assert!(entries.iter().any(|e| e.name == "nonempty"));

        // Clean up.
        vfs.unlink(dir_ino, "child.txt").unwrap();
        vfs.rmdir(ROOT_INO, "nonempty").unwrap();
    }

    #[test]
    fn test_create_file_in_subdir() {
        // Create a file inside a subdirectory and verify read/write via inode.
        let vfs = RamFsVfs;
        let dir_ino = vfs.mkdir(ROOT_INO, "subdir").unwrap();

        let file_ino = vfs.create(dir_ino, "inner.txt").unwrap();
        assert!(file_ino > 0);

        // Write data to the file using the returned inode.
        let data = b"hello from subdir";
        let written = vfs.write(file_ino, 0, data).unwrap();
        assert_eq!(written, data.len());

        // Read it back using the same inode.
        let mut buf = [0u8; 64];
        let read = vfs.read(file_ino, 0, &mut buf).unwrap();
        assert_eq!(read, data.len());
        assert_eq!(&buf[..read], data);

        // Verify readdir on the subdir shows the file.
        let entries = vfs.readdir(dir_ino).unwrap();
        assert!(entries.iter().any(|e| e.name == "inner.txt" && !e.is_dir));

        // Verify stat on the file.
        let meta = vfs.stat(file_ino).unwrap();
        assert!(!meta.is_dir);
        assert_eq!(meta.size, data.len() as u64);

        // Clean up.
        vfs.unlink(dir_ino, "inner.txt").unwrap();
        vfs.rmdir(ROOT_INO, "subdir").unwrap();
    }

    #[test]
    fn test_readdir_root_lists_entries() {
        // Verify root directory listing contains ".", "..", and created entries.
        let vfs = RamFsVfs;
        let file_ino = vfs.create(ROOT_INO, "root_file.txt").unwrap();
        let dir_ino = vfs.mkdir(ROOT_INO, "root_dir").unwrap();

        let entries = vfs.readdir(ROOT_INO).unwrap();

        // Must contain "." and "..".
        assert!(entries.iter().any(|e| e.name == "." && e.is_dir));
        assert!(entries.iter().any(|e| e.name == ".." && e.is_dir));

        // Must contain our file and directory.
        assert!(entries
            .iter()
            .any(|e| e.name == "root_file.txt" && !e.is_dir));
        assert!(entries.iter().any(|e| e.name == "root_dir" && e.is_dir));

        // Clean up.
        vfs.unlink(ROOT_INO, "root_file.txt").unwrap();
        vfs.rmdir(ROOT_INO, "root_dir").unwrap();
    }

    #[test]
    fn test_rmdir_then_recreate() {
        // Remove a directory and recreate it with the same name.
        let vfs = RamFsVfs;
        vfs.mkdir(ROOT_INO, "recycle").unwrap();
        vfs.rmdir(ROOT_INO, "recycle").unwrap();

        // Recreate should succeed.
        let dir_ino = vfs.mkdir(ROOT_INO, "recycle").unwrap();
        assert!(dir_ino > 0);

        // Should appear in readdir exactly once (excluding . and ..).
        let entries = vfs.readdir(ROOT_INO).unwrap();
        let count = entries.iter().filter(|e| e.name == "recycle").count();
        assert_eq!(count, 1);

        vfs.rmdir(ROOT_INO, "recycle").unwrap();
    }

    #[test]
    fn test_mkdir_not_a_directory_parent() {
        // Attempting to mkdir with a regular file as parent should fail.
        let vfs = RamFsVfs;
        let file_ino = vfs.create(ROOT_INO, "not_a_dir.txt").unwrap();

        assert_eq!(vfs.mkdir(file_ino, "child"), Err(FsError::NotADirectory));

        vfs.unlink(ROOT_INO, "not_a_dir.txt").unwrap();
    }

    #[test]
    fn test_rmdir_not_a_directory() {
        // Attempting to rmdir a regular file should fail with NotADirectory.
        let vfs = RamFsVfs;
        vfs.create(ROOT_INO, "regular_file.txt").unwrap();

        assert_eq!(
            vfs.rmdir(ROOT_INO, "regular_file.txt"),
            Err(FsError::NotADirectory)
        );

        vfs.unlink(ROOT_INO, "regular_file.txt").unwrap();
    }

    #[test]
    fn test_deeply_nested_directories() {
        // Create a chain: root -> a -> b -> c
        let vfs = RamFsVfs;
        let a = vfs.mkdir(ROOT_INO, "a").unwrap();
        let b = vfs.mkdir(a, "b").unwrap();
        let c = vfs.mkdir(b, "c").unwrap();

        // Create a file at the deepest level.
        let file_ino = vfs.create(c, "deep.txt").unwrap();
        vfs.write(file_ino, 0, b"deep").unwrap();

        let mut buf = [0u8; 16];
        let n = vfs.read(file_ino, 0, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"deep");

        // Clean up in reverse order.
        vfs.unlink(c, "deep.txt").unwrap();
        vfs.rmdir(b, "c").unwrap();
        vfs.rmdir(a, "b").unwrap();
        vfs.rmdir(ROOT_INO, "a").unwrap();
    }

    #[test]
    fn test_rmdir_slot_reuse_clean_state() {
        // Verify that after rmdir, a reused slot has no stale state.
        let vfs = RamFsVfs;

        // Create a directory with a child.
        let dir_ino = vfs.mkdir(ROOT_INO, "olddir").unwrap();
        vfs.create(dir_ino, "child.txt").unwrap();
        vfs.unlink(dir_ino, "child.txt").unwrap();
        vfs.rmdir(ROOT_INO, "olddir").unwrap();

        // Create a new file -- if slot reuse is buggy, this file might
        // inherit is_dir=true from the old directory.
        let new_ino = vfs.create(ROOT_INO, "newfile.txt").unwrap();
        let meta = vfs.stat(new_ino).unwrap();
        assert!(!meta.is_dir, "reused slot should not be a directory");

        vfs.unlink(ROOT_INO, "newfile.txt").unwrap();
    }

    #[test]
    fn test_rmdir_empty_after_remove() {
        // Verify that after removing all children and rmdir, the
        // directory is gone from readdir.
        let vfs = RamFsVfs;
        let dir_ino = vfs.mkdir(ROOT_INO, "muldir").unwrap();
        vfs.create(dir_ino, "a.txt").unwrap();
        vfs.create(dir_ino, "b.txt").unwrap();

        // Remove children.
        vfs.unlink(dir_ino, "a.txt").unwrap();
        vfs.unlink(dir_ino, "b.txt").unwrap();

        // Now rmdir should succeed.
        vfs.rmdir(ROOT_INO, "muldir").unwrap();

        // Verify it's gone.
        let entries = vfs.readdir(ROOT_INO).unwrap();
        assert!(!entries.iter().any(|e| e.name == "muldir"));
    }
}
