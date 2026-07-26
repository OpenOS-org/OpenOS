//! ext2 read-only filesystem implementation.
//!
//! Provides read-only access to ext2-formatted block devices. Supports:
//! - Superblock validation
//! - Block group descriptor parsing
//! - Inode reading with direct and indirect block pointers
//! - Directory traversal and file reading
//! - Full `FileSystem` trait implementation
//!
//! ## Limitations
//!
//! - Read-only (write operations return `NotSupported`)
//! - Assumes 1024-byte block size (standard ext2)
//! - No symbolic link support
//! - No extended attributes

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use super::block_cache;
use super::vfs::{DirEntry, FileSystem, FsError, InodeMeta, OpenFlags};

/// ext2 magic number in superblock.
const EXT2_MAGIC: u16 = 0xEF53;

/// Standard block size (1024 bytes).
const BLOCK_SIZE: u32 = 1024;

/// Sector size for the block cache.
const SECTOR_SIZE: u32 = 512;

/// Sectors per block (1024 / 512).
const SECTORS_PER_BLOCK: u32 = BLOCK_SIZE / SECTOR_SIZE;

/// Inode size in ext2 (128 bytes for revision 0).
const INODE_SIZE: u16 = 128;

/// Root inode number.
const ROOT_INODE: u32 = 2;

/// Inode mode: directory type mask.
const S_IFDIR: u16 = 0x4000;

/// Inode mode: regular file type mask.
const S_IFREG: u16 = 0x8000;

/// Number of direct block pointers in an inode.
const DIRECT_BLOCKS: usize = 12;

/// Directory entry file types.
const EXT2_FT_REG_FILE: u8 = 1;
const EXT2_FT_DIR: u8 = 2;

/// Read a 16-bit value from a byte slice at the given offset (little-endian).
fn read_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

/// Read a 32-bit value from a byte slice at the given offset (little-endian).
fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

/// ext2 superblock structure (on-disk layout).
///
/// Located at byte offset 1024 from the start of the partition.
/// All fields are little-endian.
#[derive(Debug, Clone)]
struct Superblock {
    /// Total number of inodes.
    inodes_count: u32,
    /// Total number of blocks.
    blocks_count: u32,
    /// Number of blocks reserved for superuser.
    r_blocks_count: u32,
    /// Number of free blocks.
    free_blocks_count: u32,
    /// Number of free inodes.
    free_inodes_count: u32,
    /// First data block (0 for block size > 1024, 1 for 1024).
    first_data_block: u32,
    /// Block size shift: block size = 1024 << `log_block_size`.
    log_block_size: u32,
    /// Fragment size shift (unused in modern ext2).
    log_frag_size: u32,
    /// Number of blocks per group.
    blocks_per_group: u32,
    /// Number of fragments per group.
    frags_per_group: u32,
    /// Number of inodes per group.
    inodes_per_group: u32,
    /// Mount time (for checking).
    mtime: u32,
    /// Write time.
    wtime: u32,
    /// Number of mounts since last check.
    mnt_count: u16,
    /// Maximum mounts before check.
    max_mnt_count: u16,
    /// Magic number (should be 0xEF53).
    magic: u16,
    /// File system state.
    state: u16,
    /// Error handling method.
    errors: u16,
    /// Minor revision level.
    minor_rev_level: u16,
    /// Last check time.
    lastcheck: u32,
    /// Maximum time between checks.
    checkinterval: u32,
    /// Creator OS ID.
    creator_os: u32,
    /// Revision level.
    rev_level: u32,
    /// Default user ID for reserved blocks.
    def_resuid: u16,
    /// Default group ID for reserved blocks.
    def_resgid: u16,
    // --- EXT2_DYNAMIC_REV fields ---
    /// First non-reserved inode.
    first_ino: u32,
    /// Inode size (128 for rev 0, >= 128 for rev 1).
    inode_size: u16,
    /// Block group number of this superblock.
    block_group_nr: u16,
    /// Feature compatibility flags.
    feature_compat: u32,
    /// Feature incompatibility flags.
    feature_incompat: u32,
    /// Read-only feature flags.
    feature_ro_compat: u32,
}

impl Superblock {
    /// Parse a superblock from raw 1024-byte data.
    fn from_bytes(data: &[u8; 1024]) -> Option<Self> {
        let magic = read_u16(data, 56);
        if magic != EXT2_MAGIC {
            return None;
        }

        Some(Self {
            inodes_count: read_u32(data, 0),
            blocks_count: read_u32(data, 4),
            r_blocks_count: read_u32(data, 8),
            free_blocks_count: read_u32(data, 12),
            free_inodes_count: read_u32(data, 16),
            first_data_block: read_u32(data, 20),
            log_block_size: read_u32(data, 24),
            log_frag_size: read_u32(data, 28),
            blocks_per_group: read_u32(data, 32),
            frags_per_group: read_u32(data, 36),
            inodes_per_group: read_u32(data, 40),
            mtime: read_u32(data, 44),
            wtime: read_u32(data, 48),
            mnt_count: read_u16(data, 52),
            max_mnt_count: read_u16(data, 54),
            magic,
            state: read_u16(data, 58),
            errors: read_u16(data, 60),
            minor_rev_level: read_u16(data, 62),
            lastcheck: read_u32(data, 64),
            checkinterval: read_u32(data, 68),
            creator_os: read_u32(data, 72),
            rev_level: read_u32(data, 76),
            def_resuid: read_u16(data, 80),
            def_resgid: read_u16(data, 82),
            first_ino: read_u32(data, 84),
            inode_size: read_u16(data, 88),
            block_group_nr: read_u16(data, 90),
            feature_compat: read_u32(data, 92),
            feature_incompat: read_u32(data, 96),
            feature_ro_compat: read_u32(data, 100),
        })
    }
}

/// Block group descriptor (on-disk layout, 32 bytes).
///
/// Describes a single block group's bitmap locations and statistics.
#[derive(Debug, Clone, Copy)]
struct BlockGroupDescriptor {
    /// Block number of the block usage bitmap.
    block_bitmap: u32,
    /// Block number of the inode usage bitmap.
    inode_bitmap: u32,
    /// Block number of the inode table.
    inode_table: u32,
    /// Number of free blocks in this group.
    free_blocks_count: u16,
    /// Number of free inodes in this group.
    free_inodes_count: u16,
    /// Number of directories in this group.
    used_dirs_count: u16,
}

impl BlockGroupDescriptor {
    /// Parse a block group descriptor from raw 32-byte data.
    fn from_bytes(data: &[u8]) -> Self {
        Self {
            block_bitmap: read_u32(data, 0),
            inode_bitmap: read_u32(data, 4),
            inode_table: read_u32(data, 8),
            free_blocks_count: read_u16(data, 12),
            free_inodes_count: read_u16(data, 14),
            used_dirs_count: read_u16(data, 16),
        }
    }
}

/// On-disk inode structure (128 bytes).
///
/// Contains metadata and block pointers for a file or directory.
#[derive(Debug, Clone)]
struct Inode {
    /// File mode (type and permissions).
    mode: u16,
    /// Owner user ID.
    uid: u16,
    /// File size in bytes (low 32 bits).
    size_low: u32,
    /// Last access time.
    atime: u32,
    /// Creation time.
    ctime: u32,
    /// Last modification time.
    mtime: u32,
    /// Deletion time.
    dtime: u32,
    /// Owner group ID.
    gid: u16,
    /// Hard link count.
    nlink: u16,
    /// Number of 512-byte blocks used.
    blocks: u32,
    /// Block pointers: 12 direct + indirect + double indirect + triple indirect.
    block: [u32; 15],
    /// File size high 32 bits (only for regular files in rev 1+).
    size_high: u32,
}

impl Inode {
    /// Parse an inode from raw 128-byte data.
    fn from_bytes(data: &[u8; 128]) -> Self {
        let mut block = [0u32; 15];
        for (i, slot) in block.iter_mut().enumerate() {
            *slot = read_u32(data, 40 + i * 4);
        }

        Self {
            mode: read_u16(data, 0),
            uid: read_u16(data, 2),
            size_low: read_u32(data, 4),
            atime: read_u32(data, 8),
            ctime: read_u32(data, 12),
            mtime: read_u32(data, 16),
            dtime: read_u32(data, 20),
            gid: read_u16(data, 24),
            nlink: read_u16(data, 26),
            blocks: read_u32(data, 28),
            block,
            size_high: read_u32(data, 108),
        }
    }

    /// Return the full 64-bit file size.
    fn size(&self) -> u64 {
        if self.is_dir() {
            u64::from(self.size_low)
        } else {
            (u64::from(self.size_high) << 32) | u64::from(self.size_low)
        }
    }

    /// Check if this inode represents a directory.
    fn is_dir(&self) -> bool {
        (self.mode & 0xF000) == S_IFDIR
    }

    /// Check if this inode represents a regular file.
    fn is_reg(&self) -> bool {
        (self.mode & 0xF000) == S_IFREG
    }
}

/// ext2 directory entry (variable-length, on-disk).
#[derive(Debug, Clone)]
struct DirEntryRaw {
    /// Inode number (0 if unused).
    inode: u32,
    /// Total entry size in bytes (must be 4-byte aligned).
    rec_len: u16,
    /// Name length in bytes.
    name_len: u8,
    /// File type indicator.
    file_type: u8,
    /// Entry name (not null-terminated on disk).
    name: Vec<u8>,
}

/// Open file descriptor tracking for the ext2 filesystem.
struct OpenFile {
    /// Inode number.
    inode_num: u32,
    /// Current read offset.
    offset: u64,
}

/// ext2 filesystem instance.
///
/// Holds the parsed superblock, block group descriptors, and provides
/// read-only access to files and directories on the block device.
pub struct Ext2Fs {
    /// Block device index.
    device_idx: usize,
    /// Parsed superblock.
    superblock: Superblock,
    /// Block group descriptors.
    group_descriptors: Vec<BlockGroupDescriptor>,
    /// Block size in bytes (computed from superblock).
    block_size: u32,
    /// Open file descriptors indexed by inode number.
    open_files: spin::Mutex<alloc::collections::BTreeMap<u64, OpenFile>>,
}

impl Ext2Fs {
    /// Open an ext2 filesystem on the given block device.
    ///
    /// Reads and validates the superblock at offset 1024, then reads the
    /// block group descriptor table. Returns `Err(())` if the device is
    /// not a valid ext2 filesystem.
    ///
    /// # Errors
    ///
    /// Returns `Err(())` if the superblock is invalid or the device cannot be read.
    pub fn open(device_idx: usize) -> Result<Self, ()> {
        // Read superblock: located at byte offset 1024 = LBA 2.
        let sb_sector0 = block_cache::read_cached(device_idx, 2).ok_or(())?;
        let sb_sector1 = block_cache::read_cached(device_idx, 3).ok_or(())?;

        let mut sb_data = [0u8; 1024];
        sb_data[..512].copy_from_slice(&sb_sector0);
        sb_data[512..].copy_from_slice(&sb_sector1);

        let superblock = Superblock::from_bytes(&sb_data).ok_or(())?;

        // Compute block size: 1024 << log_block_size.
        let block_size = 1024u32.checked_shl(superblock.log_block_size).ok_or(())?;

        // Read block group descriptor table.
        // For block_size == 1024, the BGDT starts at block 2 (byte offset 2048).
        let bgdt_block = if block_size <= 1024 { 2 } else { 1 };
        let num_groups = superblock
            .blocks_count
            .div_ceil(superblock.blocks_per_group);
        let bgdt_bytes = Self::read_block_static(device_idx, bgdt_block, block_size)?;

        let mut group_descriptors = Vec::new();
        for i in 0..num_groups as usize {
            let offset = i * 32; // Each descriptor is 32 bytes.
            if offset + 32 > bgdt_bytes.len() {
                return Err(());
            }
            group_descriptors.push(BlockGroupDescriptor::from_bytes(&bgdt_bytes[offset..]));
        }

        crate::serial_println!(
            "[OK] ext2 filesystem opened on device {}: {} inodes, {} blocks, block_size={}",
            device_idx,
            superblock.inodes_count,
            superblock.blocks_count,
            block_size
        );

        Ok(Self {
            device_idx,
            superblock,
            group_descriptors,
            block_size,
            open_files: spin::Mutex::new(alloc::collections::BTreeMap::new()),
        })
    }

    /// Read a block from the device into a buffer.
    ///
    /// A block may span multiple sectors. Reads each sector via the block cache.
    fn read_block_static(
        device_idx: usize,
        block_num: u32,
        block_size: u32,
    ) -> Result<Vec<u8>, ()> {
        let sectors_per_block = block_size / SECTOR_SIZE;
        let start_lba = u64::from(block_num) * u64::from(sectors_per_block);
        let mut data = vec![0u8; block_size as usize];

        for i in 0..sectors_per_block {
            let lba = start_lba + u64::from(i);
            let sector_data = block_cache::read_cached(device_idx, lba).ok_or(())?;
            let offset = (i * SECTOR_SIZE) as usize;
            data[offset..offset + SECTOR_SIZE as usize].copy_from_slice(&sector_data);
        }

        Ok(data)
    }

    /// Read a block from the device (instance method).
    fn read_block(&self, block_num: u32) -> Result<Vec<u8>, ()> {
        Self::read_block_static(self.device_idx, block_num, self.block_size)
    }

    /// Read an inode by its number.
    ///
    /// Calculates which block group the inode belongs to, reads the inode
    /// from that group's inode table, and parses it.
    fn read_inode(&self, inode_num: u32) -> Result<Inode, ()> {
        if inode_num == 0 {
            return Err(());
        }

        // Inode numbers are 1-based.
        let ino_index = inode_num - 1;
        let group = ino_index / self.superblock.inodes_per_group;
        let index_in_group = ino_index % self.superblock.inodes_per_group;

        let group_idx = group as usize;
        if group_idx >= self.group_descriptors.len() {
            return Err(());
        }

        let desc = self.group_descriptors[group_idx];
        let inode_size = self.superblock.inode_size.max(INODE_SIZE);
        let offset_in_table = u64::from(index_in_group) * u64::from(inode_size);

        // Which block of the inode table contains this inode?
        let block_in_table = (offset_in_table / u64::from(self.block_size)) as u32;
        let offset_in_block = (offset_in_table % u64::from(self.block_size)) as usize;

        let block_num = desc.inode_table + block_in_table;
        let block_data = self.read_block(block_num)?;

        if offset_in_block + INODE_SIZE as usize > block_data.len() {
            return Err(());
        }

        let mut inode_data = [0u8; 128];
        inode_data.copy_from_slice(&block_data[offset_in_block..offset_in_block + 128]);

        Ok(Inode::from_bytes(&inode_data))
    }

    /// Resolve a block pointer from an inode, handling indirect blocks.
    ///
    /// Given a logical block index within a file, returns the physical block number.
    /// Supports direct (0-11), single indirect (12), double indirect (13),
    /// and triple indirect (14) block pointers.
    fn resolve_block(&self, inode: &Inode, logical_block: u32) -> Result<u32, ()> {
        let entries_per_block = self.block_size / 4;

        if logical_block < DIRECT_BLOCKS as u32 {
            // Direct block.
            Ok(inode.block[logical_block as usize])
        } else if logical_block < DIRECT_BLOCKS as u32 + entries_per_block {
            // Single indirect.
            let index = logical_block - DIRECT_BLOCKS as u32;
            if inode.block[12] == 0 {
                return Ok(0);
            }
            self.read_indirect(inode.block[12], index)
        } else if logical_block
            < DIRECT_BLOCKS as u32 + entries_per_block + entries_per_block * entries_per_block
        {
            // Double indirect.
            let base = DIRECT_BLOCKS as u32 + entries_per_block;
            let index = logical_block - base;
            if inode.block[13] == 0 {
                return Ok(0);
            }
            self.read_double_indirect(inode.block[13], index)
        } else {
            // Triple indirect.
            let base =
                DIRECT_BLOCKS as u32 + entries_per_block + entries_per_block * entries_per_block;
            let index = logical_block - base;
            if inode.block[14] == 0 {
                return Ok(0);
            }
            self.read_triple_indirect(inode.block[14], index)
        }
    }

    /// Read a block number from a single indirect block.
    fn read_indirect(&self, block_num: u32, index: u32) -> Result<u32, ()> {
        if block_num == 0 {
            return Ok(0);
        }
        let data = self.read_block(block_num)?;
        let offset = (index * 4) as usize;
        if offset + 4 > data.len() {
            return Err(());
        }
        Ok(read_u32(&data, offset))
    }

    /// Read a block number from a double indirect block.
    fn read_double_indirect(&self, block_num: u32, index: u32) -> Result<u32, ()> {
        if block_num == 0 {
            return Ok(0);
        }
        let entries_per_block = self.block_size / 4;
        let first = index / entries_per_block;
        let second = index % entries_per_block;

        let data = self.read_block(block_num)?;
        let offset = (first * 4) as usize;
        if offset + 4 > data.len() {
            return Err(());
        }
        let indirect_block = read_u32(&data, offset);
        self.read_indirect(indirect_block, second)
    }

    /// Read a block number from a triple indirect block.
    fn read_triple_indirect(&self, block_num: u32, index: u32) -> Result<u32, ()> {
        if block_num == 0 {
            return Ok(0);
        }
        let entries_per_block = self.block_size / 4;
        let first = index / (entries_per_block * entries_per_block);
        let remainder = index % (entries_per_block * entries_per_block);

        let data = self.read_block(block_num)?;
        let offset = (first * 4) as usize;
        if offset + 4 > data.len() {
            return Err(());
        }
        let double_block = read_u32(&data, offset);
        self.read_double_indirect(double_block, remainder)
    }

    /// Parse directory entries from a directory inode's data blocks.
    fn read_dir_entries(&self, inode: &Inode) -> Result<Vec<DirEntryRaw>, ()> {
        if !inode.is_dir() {
            return Err(());
        }

        let file_size = inode.size() as u32;
        let mut entries = Vec::new();
        let mut bytes_read: u32 = 0;
        let entries_per_block = self.block_size / 4;

        for block_idx in 0.. {
            if bytes_read >= file_size {
                break;
            }

            let physical_block = self.resolve_block(inode, block_idx)?;
            if physical_block == 0 {
                bytes_read += self.block_size;
                continue;
            }

            let block_data = self.read_block(physical_block)?;
            let mut offset = 0usize;

            while offset + 8 <= self.block_size as usize && bytes_read < file_size {
                let inode_num = read_u32(&block_data, offset);
                let rec_len = read_u16(&block_data, offset + 4);
                let name_len = block_data[offset + 6] as usize;
                let file_type = block_data[offset + 7];

                if rec_len == 0 {
                    // Corrupted directory, stop parsing this block.
                    break;
                }

                if inode_num != 0 && name_len > 0 {
                    let name_start = offset + 8;
                    let name_end = name_start + name_len;
                    if name_end <= self.block_size as usize {
                        let name = block_data[name_start..name_end].to_vec();
                        entries.push(DirEntryRaw {
                            inode: inode_num,
                            rec_len,
                            name_len: name_len as u8,
                            file_type,
                            name,
                        });
                    }
                }

                let rec_len = rec_len as usize;
                offset += rec_len;
                bytes_read += rec_len as u32;
            }
        }

        Ok(entries)
    }

    /// Find a directory entry by name within a directory inode.
    fn find_entry(&self, dir_inode: &Inode, name: &str) -> Option<u32> {
        let entries = self.read_dir_entries(dir_inode).ok()?;
        for entry in &entries {
            if entry.name == name.as_bytes() {
                return Some(entry.inode);
            }
        }
        None
    }

    /// Walk a path and return the inode number of the target.
    ///
    /// Supports absolute paths starting with '/'. Each path component is
    /// resolved by looking up the name in the parent directory.
    fn resolve_path(&self, path: &str) -> Result<u32, ()> {
        let path = path.trim_start_matches('/');
        if path.is_empty() {
            return Ok(ROOT_INODE);
        }

        let mut current_inode_num = ROOT_INODE;
        let components: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();

        for component in components {
            let inode = self.read_inode(current_inode_num)?;
            if !inode.is_dir() {
                return Err(());
            }

            match self.find_entry(&inode, component) {
                Some(next_inode) => current_inode_num = next_inode,
                None => return Err(()),
            }
        }

        Ok(current_inode_num)
    }

    /// Read data from an inode at the given offset.
    ///
    /// Follows direct and indirect block pointers to read the requested bytes.
    fn read_inode_data(&self, inode: &Inode, offset: u64, buf: &mut [u8]) -> Result<usize, ()> {
        let file_size = inode.size();
        if offset >= file_size {
            return Ok(0); // EOF
        }

        let available = file_size - offset;
        let to_read = (buf.len() as u64).min(available) as usize;
        if to_read == 0 {
            return Ok(0);
        }

        let entries_per_block = self.block_size / 4;
        let mut bytes_read = 0usize;

        while bytes_read < to_read {
            let file_offset = offset + bytes_read as u64;
            let logical_block = (file_offset / u64::from(self.block_size)) as u32;
            let offset_in_block = (file_offset % u64::from(self.block_size)) as usize;

            let physical_block = self.resolve_block(inode, logical_block)?;
            if physical_block == 0 {
                // Sparse block — fill with zeros.
                let remaining = to_read - bytes_read;
                let available_in_block = self.block_size as usize - offset_in_block;
                let fill = remaining.min(available_in_block);
                for byte in &mut buf[bytes_read..bytes_read + fill] {
                    *byte = 0;
                }
                bytes_read += fill;
                continue;
            }

            let block_data = self.read_block(physical_block)?;
            let available_in_block = self.block_size as usize - offset_in_block;
            let remaining = to_read - bytes_read;
            let copy_len = remaining.min(available_in_block);

            buf[bytes_read..bytes_read + copy_len]
                .copy_from_slice(&block_data[offset_in_block..offset_in_block + copy_len]);
            bytes_read += copy_len;
        }

        Ok(bytes_read)
    }
}

impl FileSystem for Ext2Fs {
    fn open(&self, path: &str, _flags: OpenFlags) -> Result<u64, FsError> {
        let inode_num = self.resolve_path(path).map_err(|()| FsError::NotFound)?;
        let ino = u64::from(inode_num);

        let mut fds = self.open_files.lock();
        fds.insert(
            ino,
            OpenFile {
                inode_num,
                offset: 0,
            },
        );

        Ok(ino)
    }

    fn close(&self, ino: u64) -> Result<(), FsError> {
        let mut fds = self.open_files.lock();
        fds.remove(&ino);
        Ok(())
    }

    fn read(&self, ino: u64, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        let inode_num = u32::try_from(ino).map_err(|_| FsError::BadFileDescriptor)?;
        let inode = self.read_inode(inode_num).map_err(|()| FsError::IoError)?;

        self.read_inode_data(&inode, offset, buf)
            .map_err(|()| FsError::IoError)
    }

    fn write(&self, _ino: u64, _offset: u64, _data: &[u8]) -> Result<usize, FsError> {
        Err(FsError::NotSupported)
    }

    fn stat(&self, ino: u64) -> Result<InodeMeta, FsError> {
        let inode_num = u32::try_from(ino).map_err(|_| FsError::BadFileDescriptor)?;
        let inode = self.read_inode(inode_num).map_err(|()| FsError::NotFound)?;

        Ok(InodeMeta {
            ino,
            is_dir: inode.is_dir(),
            size: inode.size(),
            nlink: u32::from(inode.nlink),
        })
    }

    fn readdir(&self, dir_ino: u64) -> Result<Vec<DirEntry>, FsError> {
        let inode_num = u32::try_from(dir_ino).map_err(|_| FsError::BadFileDescriptor)?;
        let inode = self.read_inode(inode_num).map_err(|()| FsError::NotFound)?;

        if !inode.is_dir() {
            return Err(FsError::NotSupported);
        }

        let raw_entries = self
            .read_dir_entries(&inode)
            .map_err(|()| FsError::IoError)?;

        let mut entries = Vec::with_capacity(raw_entries.len());
        for entry in &raw_entries {
            if let Ok(name) = core::str::from_utf8(&entry.name) {
                let entry_ino = u64::from(entry.inode);
                let is_dir = entry.file_type == EXT2_FT_DIR;
                entries.push(DirEntry {
                    name: String::from(name),
                    ino: entry_ino,
                    is_dir,
                });
            }
        }

        Ok(entries)
    }

    fn create(&self, _parent_ino: u64, _name: &str) -> Result<u64, FsError> {
        Err(FsError::NotSupported)
    }

    fn unlink(&self, _parent_ino: u64, _name: &str) -> Result<(), FsError> {
        Err(FsError::NotSupported)
    }
}
