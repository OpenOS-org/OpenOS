//! ext2 filesystem implementation with read and write support.
//!
//! Provides read/write access to ext2-formatted block devices. Supports:
//! - Superblock validation
//! - Block group descriptor parsing
//! - Inode reading with direct and indirect block pointers
//! - Directory traversal and file reading
//! - Block and inode allocation/deallocation via bitmap scanning
//! - File creation, writing (with block allocation), and unlinking
//! - Full `FileSystem` trait implementation
//!
//! ## Limitations
//!
//! - Assumes 1024-byte block size (standard ext2)
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

/// Inode mode: symbolic link type mask.
const S_IFLNK: u16 = 0xA000;

/// Number of direct block pointers in an inode.
const DIRECT_BLOCKS: usize = 12;

/// Directory entry file types.
const EXT2_FT_REG_FILE: u8 = 1;
const EXT2_FT_DIR: u8 = 2;
const EXT2_FT_SYMLINK: u8 = 7;

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

/// Write a 32-bit value to a byte slice at the given offset (little-endian).
fn write_u32(data: &mut [u8], offset: usize, value: u32) {
    let bytes = value.to_le_bytes();
    data[offset..offset + 4].copy_from_slice(&bytes);
}

/// Write a 16-bit value to a byte slice at the given offset (little-endian).
fn write_u16_bytes(data: &mut [u8], offset: usize, value: u16) {
    let bytes = value.to_le_bytes();
    data[offset..offset + 2].copy_from_slice(&bytes);
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

    /// Check if this inode represents a symbolic link.
    fn is_symlink(&self) -> bool {
        (self.mode & 0xF000) == S_IFLNK
    }

    /// Serialize this inode to a 128-byte array (on-disk format).
    fn to_bytes(&self) -> [u8; 128] {
        let mut data = [0u8; 128];
        write_u16_bytes(&mut data, 0, self.mode);
        write_u16_bytes(&mut data, 2, self.uid);
        write_u32(&mut data, 4, self.size_low);
        write_u32(&mut data, 8, self.atime);
        write_u32(&mut data, 12, self.ctime);
        write_u32(&mut data, 16, self.mtime);
        write_u32(&mut data, 20, self.dtime);
        write_u16_bytes(&mut data, 24, self.gid);
        write_u16_bytes(&mut data, 26, self.nlink);
        write_u32(&mut data, 28, self.blocks);
        for (i, &blk) in self.block.iter().enumerate() {
            write_u32(&mut data, 40 + i * 4, blk);
        }
        write_u32(&mut data, 108, self.size_high);
        data
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
    #[allow(dead_code)]
    inode_num: u32,
    /// Current read offset.
    #[allow(dead_code)]
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

    /// Write a block to the device via the block cache.
    fn write_block(&self, block_num: u32, data: &[u8]) -> Result<(), ()> {
        let sectors_per_block = self.block_size / SECTOR_SIZE;
        let start_lba = u64::from(block_num) * u64::from(sectors_per_block);

        for i in 0..sectors_per_block {
            let lba = start_lba + u64::from(i);
            let offset = (i * SECTOR_SIZE) as usize;
            let mut sector = [0u8; SECTOR_SIZE as usize];
            sector.copy_from_slice(&data[offset..offset + SECTOR_SIZE as usize]);
            block_cache::write_cached(self.device_idx, lba, &sector)?;
        }
        Ok(())
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

    /// Write data to an inode at the given offset, allocating new blocks as needed.
    fn write_inode_data(
        &self,
        inode: &mut Inode,
        inode_num: u32,
        offset: u64,
        data: &[u8],
    ) -> Result<usize, ()> {
        if data.is_empty() {
            return Ok(0);
        }

        let end_offset = offset + data.len() as u64;
        let mut bytes_written = 0usize;

        while bytes_written < data.len() {
            let file_offset = offset + bytes_written as u64;
            let logical_block = (file_offset / u64::from(self.block_size)) as u32;
            let offset_in_block = (file_offset % u64::from(self.block_size)) as usize;

            let physical_block = match self.resolve_block(inode, logical_block)? {
                0 => {
                    // Allocate a new block for this logical position.
                    let new_block = self.alloc_block().ok_or(())?;
                    self.set_block_pointer(inode, logical_block, new_block)?;
                    new_block
                }
                block => block,
            };

            let available_in_block = self.block_size as usize - offset_in_block;
            let remaining = data.len() - bytes_written;
            let copy_len = remaining.min(available_in_block);

            // Read-modify-write the block.
            let mut block_data = if offset_in_block == 0 && copy_len == self.block_size as usize {
                vec![0u8; self.block_size as usize]
            } else {
                self.read_block(physical_block)?
            };

            block_data[offset_in_block..offset_in_block + copy_len]
                .copy_from_slice(&data[bytes_written..bytes_written + copy_len]);
            self.write_block(physical_block, &block_data)?;
            bytes_written += copy_len;
        }

        // Update inode size if we extended the file.
        if end_offset > inode.size() {
            let new_size = end_offset;
            inode.size_low = (new_size & 0xFFFF_FFFF) as u32;
            inode.size_high = ((new_size >> 32) & 0xFFFF_FFFF) as u32;
        }

        // Update block count (each block = 2 sectors of 512 bytes).
        let blocks_512 = inode.size().div_ceil(512) as u32;
        inode.blocks = blocks_512;

        // Write the updated inode back.
        self.write_inode(inode_num, inode)?;

        Ok(bytes_written)
    }

    /// Set a block pointer for a logical block index within an inode.
    ///
    /// Allocates indirect blocks as needed.
    fn set_block_pointer(
        &self,
        inode: &mut Inode,
        logical_block: u32,
        physical_block: u32,
    ) -> Result<(), ()> {
        let entries_per_block = self.block_size / 4;

        if logical_block < DIRECT_BLOCKS as u32 {
            inode.block[logical_block as usize] = physical_block;
            Ok(())
        } else if logical_block < DIRECT_BLOCKS as u32 + entries_per_block {
            // Single indirect.
            let index = logical_block - DIRECT_BLOCKS as u32;
            if inode.block[12] == 0 {
                let new_indirect = self.alloc_block().ok_or(())?;
                inode.block[12] = new_indirect;
                // Zero the indirect block.
                self.write_block(new_indirect, &vec![0u8; self.block_size as usize])?;
            }
            self.write_indirect_entry(inode.block[12], index, physical_block)
        } else if logical_block
            < DIRECT_BLOCKS as u32 + entries_per_block + entries_per_block * entries_per_block
        {
            // Double indirect.
            let base = DIRECT_BLOCKS as u32 + entries_per_block;
            let index = logical_block - base;
            if inode.block[13] == 0 {
                let new_double = self.alloc_block().ok_or(())?;
                inode.block[13] = new_double;
                self.write_block(new_double, &vec![0u8; self.block_size as usize])?;
            }
            self.set_double_indirect_entry(inode.block[13], index, physical_block)
        } else {
            // Triple indirect.
            let base =
                DIRECT_BLOCKS as u32 + entries_per_block + entries_per_block * entries_per_block;
            let index = logical_block - base;
            if inode.block[14] == 0 {
                let new_triple = self.alloc_block().ok_or(())?;
                inode.block[14] = new_triple;
                self.write_block(new_triple, &vec![0u8; self.block_size as usize])?;
            }
            self.set_triple_indirect_entry(inode.block[14], index, physical_block)
        }
    }

    /// Write an entry into a single indirect block.
    fn write_indirect_entry(&self, block_num: u32, index: u32, value: u32) -> Result<(), ()> {
        let mut data = self.read_block(block_num)?;
        let offset = (index * 4) as usize;
        write_u32(&mut data, offset, value);
        self.write_block(block_num, &data)
    }

    /// Write an entry into a double indirect block.
    fn set_double_indirect_entry(&self, block_num: u32, index: u32, value: u32) -> Result<(), ()> {
        let entries_per_block = self.block_size / 4;
        let first = index / entries_per_block;
        let second = index % entries_per_block;

        let mut data = self.read_block(block_num)?;
        let offset = (first * 4) as usize;
        let indirect_block = read_u32(&data, offset);

        if indirect_block == 0 {
            let new_indirect = self.alloc_block().ok_or(())?;
            write_u32(&mut data, offset, new_indirect);
            self.write_block(block_num, &data)?;
            self.write_block(new_indirect, &vec![0u8; self.block_size as usize])?;
            self.write_indirect_entry(new_indirect, second, value)
        } else {
            self.write_indirect_entry(indirect_block, second, value)
        }
    }

    /// Write an entry into a triple indirect block.
    fn set_triple_indirect_entry(&self, block_num: u32, index: u32, value: u32) -> Result<(), ()> {
        let entries_per_block = self.block_size / 4;
        let first = index / (entries_per_block * entries_per_block);
        let remainder = index % (entries_per_block * entries_per_block);

        let mut data = self.read_block(block_num)?;
        let offset = (first * 4) as usize;
        let double_block = read_u32(&data, offset);

        if double_block == 0 {
            let new_double = self.alloc_block().ok_or(())?;
            write_u32(&mut data, offset, new_double);
            self.write_block(block_num, &data)?;
            self.write_block(new_double, &vec![0u8; self.block_size as usize])?;
            self.set_double_indirect_entry(new_double, remainder, value)
        } else {
            self.set_double_indirect_entry(double_block, remainder, value)
        }
    }

    /// Write an inode back to disk.
    fn write_inode(&self, inode_num: u32, inode: &Inode) -> Result<(), ()> {
        if inode_num == 0 {
            return Err(());
        }

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

        let block_in_table = (offset_in_table / u64::from(self.block_size)) as u32;
        let offset_in_block = (offset_in_table % u64::from(self.block_size)) as usize;

        let block_num = desc.inode_table + block_in_table;
        let mut block_data = self.read_block(block_num)?;

        let inode_bytes = inode.to_bytes();
        block_data[offset_in_block..offset_in_block + 128].copy_from_slice(&inode_bytes);

        self.write_block(block_num, &block_data)
    }

    /// Allocate a free block by scanning the block bitmap.
    ///
    /// Returns the physical block number of the allocated block, or `None`
    /// if no free blocks are available.
    fn alloc_block(&self) -> Option<u32> {
        // SAFETY: We need mutable access to superblock/group_descriptors for
        // updating free counts. We use interior mutability via the caller
        // pattern — the methods that call alloc_block hold &self, but we
        // need to write back metadata. We'll write the bitmap and metadata
        // directly to disk and return the block number.
        let blocks_per_group = self.superblock.blocks_per_group;
        let num_groups = self.group_descriptors.len();

        for group_idx in 0..num_groups {
            let desc = self.group_descriptors[group_idx];
            if desc.free_blocks_count == 0 {
                continue;
            }

            let mut bitmap_data = self.read_block(desc.block_bitmap).ok()?;
            let bits_per_block = self.block_size * 8;

            for byte_idx in 0..(bits_per_block as usize / 8) {
                if byte_idx >= bitmap_data.len() {
                    break;
                }
                let byte = bitmap_data[byte_idx];
                if byte == 0xFF {
                    continue;
                }
                // Find the first zero bit.
                for bit in 0..8u32 {
                    if (byte & (1 << bit)) == 0 {
                        let block_in_group = byte_idx as u32 * 8 + bit;
                        if block_in_group >= blocks_per_group {
                            return None;
                        }

                        // Compute the absolute block number.
                        let absolute_block = self.superblock.first_data_block
                            + group_idx as u32 * blocks_per_group
                            + block_in_group;

                        // Mark the block as used in the bitmap.
                        bitmap_data[byte_idx] |= 1 << bit;
                        self.write_block(desc.block_bitmap, &bitmap_data).ok()?;

                        return Some(absolute_block);
                    }
                }
            }
        }

        None
    }

    /// Free a block by marking it as available in the block bitmap.
    fn free_block(&self, block: u32) -> Result<(), ()> {
        if block == 0 {
            return Ok(());
        }

        let blocks_per_group = self.superblock.blocks_per_group;
        let relative = block - self.superblock.first_data_block;
        let group_idx = (relative / blocks_per_group) as usize;
        let block_in_group = relative % blocks_per_group;

        if group_idx >= self.group_descriptors.len() {
            return Err(());
        }

        let desc = self.group_descriptors[group_idx];
        let mut bitmap = self.read_block(desc.block_bitmap)?;
        let byte_idx = (block_in_group / 8) as usize;
        let bit = block_in_group % 8;

        if byte_idx >= bitmap.len() {
            return Err(());
        }

        bitmap[byte_idx] &= !(1 << bit);
        self.write_block(desc.block_bitmap, &bitmap)
    }

    /// Allocate a free inode by scanning the inode bitmap.
    ///
    /// Returns the inode number (1-based) of the allocated inode, or `None`
    /// if no free inodes are available.
    fn alloc_inode(&self) -> Option<u32> {
        let inodes_per_group = self.superblock.inodes_per_group;
        let num_groups = self.group_descriptors.len();

        for group_idx in 0..num_groups {
            let desc = self.group_descriptors[group_idx];
            if desc.free_inodes_count == 0 {
                continue;
            }

            let mut bitmap_data = self.read_block(desc.inode_bitmap).ok()?;
            let bits_per_block = self.block_size * 8;

            for byte_idx in 0..(bits_per_block as usize / 8) {
                if byte_idx >= bitmap_data.len() {
                    break;
                }
                let byte = bitmap_data[byte_idx];
                if byte == 0xFF {
                    continue;
                }
                for bit in 0..8u32 {
                    if (byte & (1 << bit)) == 0 {
                        let inode_in_group = byte_idx as u32 * 8 + bit;
                        if inode_in_group >= inodes_per_group {
                            return None;
                        }

                        // Inode numbers are 1-based.
                        let inode_num = group_idx as u32 * inodes_per_group + inode_in_group + 1;

                        // Skip reserved inodes (< first_ino).
                        if inode_num < self.superblock.first_ino {
                            continue;
                        }

                        // Mark the inode as used.
                        bitmap_data[byte_idx] |= 1 << bit;
                        self.write_block(desc.inode_bitmap, &bitmap_data).ok()?;

                        return Some(inode_num);
                    }
                }
            }
        }

        None
    }

    /// Free an inode by marking it as available in the inode bitmap.
    fn free_inode(&self, inode_num: u32) -> Result<(), ()> {
        if inode_num == 0 {
            return Ok(());
        }

        let inodes_per_group = self.superblock.inodes_per_group;
        let ino_index = inode_num - 1;
        let group_idx = (ino_index / inodes_per_group) as usize;
        let inode_in_group = ino_index % inodes_per_group;

        if group_idx >= self.group_descriptors.len() {
            return Err(());
        }

        let desc = self.group_descriptors[group_idx];
        let mut bitmap = self.read_block(desc.inode_bitmap)?;
        let byte_idx = (inode_in_group / 8) as usize;
        let bit = inode_in_group % 8;

        if byte_idx >= bitmap.len() {
            return Err(());
        }

        bitmap[byte_idx] &= !(1 << bit);
        self.write_block(desc.inode_bitmap, &bitmap)
    }

    /// Add a directory entry to a directory inode.
    fn add_dir_entry(
        &self,
        dir_inode: &mut Inode,
        dir_num: u32,
        entry_inode: u32,
        name: &str,
        file_type: u8,
    ) -> Result<(), ()> {
        let name_bytes = name.as_bytes();
        let name_len = name_bytes.len() as u8;
        // Entry size = 8 (header) + name_len, rounded up to 4 bytes.
        let needed = ((8 + name_bytes.len() + 3) & !3) as u16;

        let dir_size = dir_inode.size() as u32;
        let entries_per_block = self.block_size / 4;

        // Try to find space in existing blocks.
        let mut bytes_scanned: u32 = 0;
        let mut logical_block: u32 = 0;

        while bytes_scanned < dir_size {
            let physical_block = self.resolve_block(dir_inode, logical_block)?;
            if physical_block != 0 {
                let mut block_data = self.read_block(physical_block)?;
                let mut offset = 0usize;

                while offset + 8 <= self.block_size as usize {
                    let rec_len = read_u16(&block_data, offset + 4) as usize;
                    if rec_len == 0 {
                        break;
                    }

                    let entry_inode_num = read_u32(&block_data, offset);
                    let entry_name_len = block_data[offset + 6] as usize;
                    let actual_entry_size = (8 + entry_name_len + 3) & !3;

                    if entry_inode_num != 0 {
                        // Existing entry — check if there's slack space.
                        if actual_entry_size + needed as usize <= rec_len {
                            // Split this entry: shrink it and add new entry in slack.
                            let new_rec_len = actual_entry_size as u16;
                            write_u16_bytes(&mut block_data, offset + 4, new_rec_len);

                            let new_offset = offset + actual_entry_size;
                            let remaining = rec_len - actual_entry_size;
                            write_u32(&mut block_data, new_offset, entry_inode);
                            write_u16_bytes(&mut block_data, new_offset + 4, remaining as u16);
                            block_data[new_offset + 6] = name_len;
                            block_data[new_offset + 7] = file_type;
                            let name_start = new_offset + 8;
                            block_data[name_start..name_start + name_bytes.len()]
                                .copy_from_slice(name_bytes);

                            self.write_block(physical_block, &block_data)?;
                            return Ok(());
                        }
                    } else {
                        // Unused entry — check if it's big enough.
                        if rec_len >= needed as usize {
                            write_u32(&mut block_data, offset, entry_inode);
                            write_u16_bytes(&mut block_data, offset + 4, rec_len as u16);
                            block_data[offset + 6] = name_len;
                            block_data[offset + 7] = file_type;
                            let name_start = offset + 8;
                            block_data[name_start..name_start + name_bytes.len()]
                                .copy_from_slice(name_bytes);

                            self.write_block(physical_block, &block_data)?;
                            return Ok(());
                        }
                    }

                    offset += rec_len;
                    bytes_scanned += rec_len as u32;
                }
            } else {
                bytes_scanned += self.block_size;
            }
            logical_block += 1;
        }

        // No space found — allocate a new block for the directory.
        let new_block = self.alloc_block().ok_or(())?;
        let mut block_data = vec![0u8; self.block_size as usize];
        write_u32(&mut block_data, 0, entry_inode);
        write_u16_bytes(&mut block_data, 4, self.block_size as u16);
        block_data[6] = name_len;
        block_data[7] = file_type;
        block_data[8..8 + name_bytes.len()].copy_from_slice(name_bytes);

        self.write_block(new_block, &block_data)?;

        // Update the directory inode to point to this new block.
        let new_logical_block = dir_size.div_ceil(self.block_size);
        self.set_block_pointer(dir_inode, new_logical_block, new_block)?;

        // Update directory size.
        let new_size = (new_logical_block + 1) * self.block_size;
        dir_inode.size_low = new_size;
        dir_inode.blocks = new_size.div_ceil(512);

        self.write_inode(dir_num, dir_inode)
    }

    /// Find and zero out a directory entry by name, merging with adjacent free space.
    fn remove_dir_entry_from_disk(&self, dir_inode: &Inode, name: &str) -> Result<Option<u32>, ()> {
        let dir_size = dir_inode.size() as u32;
        let mut bytes_scanned: u32 = 0;
        let mut logical_block: u32 = 0;

        while bytes_scanned < dir_size {
            let physical_block = self.resolve_block(dir_inode, logical_block)?;
            if physical_block != 0 {
                let mut block_data = self.read_block(physical_block)?;
                let mut offset = 0usize;
                let mut prev_offset: Option<usize> = None;

                while offset + 8 <= self.block_size as usize {
                    let rec_len = read_u16(&block_data, offset + 4) as usize;
                    if rec_len == 0 {
                        break;
                    }

                    let entry_inode = read_u32(&block_data, offset);
                    let entry_name_len = block_data[offset + 6] as usize;
                    let entry_name = &block_data[offset + 8..offset + 8 + entry_name_len];

                    if entry_inode != 0 && entry_name == name.as_bytes() {
                        // Zero the inode field to mark entry as free.
                        write_u32(&mut block_data, offset, 0);

                        // Try to merge with the previous entry.
                        if let Some(prev_off) = prev_offset {
                            let prev_inode = read_u32(&block_data, prev_off);
                            if prev_inode == 0 {
                                let prev_rec_len = read_u16(&block_data, prev_off + 4) as usize;
                                write_u16_bytes(
                                    &mut block_data,
                                    prev_off + 4,
                                    (prev_rec_len + rec_len) as u16,
                                );
                                // The current entry is now part of the previous free space.
                                self.write_block(physical_block, &block_data)?;
                                return Ok(Some(entry_inode));
                            }
                        }

                        self.write_block(physical_block, &block_data)?;
                        return Ok(Some(entry_inode));
                    }

                    prev_offset = Some(offset);
                    offset += rec_len;
                    bytes_scanned += rec_len as u32;
                }
            } else {
                bytes_scanned += self.block_size;
            }
            logical_block += 1;
        }

        Ok(None)
    }

    /// Free all data blocks of an inode (direct, indirect, double, triple).
    fn free_inode_blocks(&self, inode: &Inode) -> Result<(), ()> {
        let entries_per_block = self.block_size / 4;

        // Free direct blocks.
        for i in 0..DIRECT_BLOCKS {
            if inode.block[i] != 0 {
                self.free_block(inode.block[i])?;
            }
        }

        // Free single indirect.
        if inode.block[12] != 0 {
            let data = self.read_block(inode.block[12])?;
            for i in 0..entries_per_block as usize {
                let block_num = read_u32(&data, i * 4);
                if block_num != 0 {
                    self.free_block(block_num)?;
                }
            }
            self.free_block(inode.block[12])?;
        }

        // Free double indirect.
        if inode.block[13] != 0 {
            let data = self.read_block(inode.block[13])?;
            for i in 0..entries_per_block as usize {
                let indirect_block = read_u32(&data, i * 4);
                if indirect_block != 0 {
                    let indirect_data = self.read_block(indirect_block)?;
                    for j in 0..entries_per_block as usize {
                        let block_num = read_u32(&indirect_data, j * 4);
                        if block_num != 0 {
                            self.free_block(block_num)?;
                        }
                    }
                    self.free_block(indirect_block)?;
                }
            }
            self.free_block(inode.block[13])?;
        }

        // Free triple indirect.
        if inode.block[14] != 0 {
            let data = self.read_block(inode.block[14])?;
            for i in 0..entries_per_block as usize {
                let double_block = read_u32(&data, i * 4);
                if double_block != 0 {
                    let double_data = self.read_block(double_block)?;
                    for j in 0..entries_per_block as usize {
                        let indirect_block = read_u32(&double_data, j * 4);
                        if indirect_block != 0 {
                            let indirect_data = self.read_block(indirect_block)?;
                            for k in 0..entries_per_block as usize {
                                let block_num = read_u32(&indirect_data, k * 4);
                                if block_num != 0 {
                                    self.free_block(block_num)?;
                                }
                            }
                            self.free_block(indirect_block)?;
                        }
                    }
                    self.free_block(double_block)?;
                }
            }
            self.free_block(inode.block[14])?;
        }

        Ok(())
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

    fn write(&self, ino: u64, offset: u64, data: &[u8]) -> Result<usize, FsError> {
        let inode_num = u32::try_from(ino).map_err(|_| FsError::BadFileDescriptor)?;
        let mut inode = self.read_inode(inode_num).map_err(|()| FsError::IoError)?;

        if !inode.is_reg() {
            return Err(FsError::NotSupported);
        }

        self.write_inode_data(&mut inode, inode_num, offset, data)
            .map_err(|()| FsError::IoError)
    }

    fn stat(&self, ino: u64) -> Result<InodeMeta, FsError> {
        let inode_num = u32::try_from(ino).map_err(|_| FsError::BadFileDescriptor)?;
        let inode = self.read_inode(inode_num).map_err(|()| FsError::NotFound)?;

        Ok(InodeMeta {
            ino,
            is_dir: inode.is_dir(),
            is_symlink: inode.is_symlink(),
            is_fifo: false,
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

    fn create(&self, parent_ino: u64, name: &str) -> Result<u64, FsError> {
        if name.is_empty() || name.len() > 255 {
            return Err(FsError::InvalidName);
        }

        let parent_num = u32::try_from(parent_ino).map_err(|_| FsError::BadFileDescriptor)?;
        let mut parent_inode = self
            .read_inode(parent_num)
            .map_err(|()| FsError::NotFound)?;

        if !parent_inode.is_dir() {
            return Err(FsError::NotSupported);
        }

        // Check if the name already exists.
        if self.find_entry(&parent_inode, name).is_some() {
            return Err(FsError::AlreadyExists);
        }

        // Allocate a new inode.
        let new_inode_num = self.alloc_inode().ok_or(FsError::NoSpace)?;

        // Initialize the new inode as a regular file.
        let new_inode = Inode {
            mode: S_IFREG | 0o644,
            uid: 0,
            size_low: 0,
            atime: 0,
            ctime: 0,
            mtime: 0,
            dtime: 0,
            gid: 0,
            nlink: 1,
            blocks: 0,
            block: [0u32; 15],
            size_high: 0,
        };
        self.write_inode(new_inode_num, &new_inode)
            .map_err(|()| FsError::IoError)?;

        // Add directory entry.
        self.add_dir_entry(
            &mut parent_inode,
            parent_num,
            new_inode_num,
            name,
            EXT2_FT_REG_FILE,
        )
        .map_err(|()| FsError::IoError)?;

        Ok(u64::from(new_inode_num))
    }

    fn unlink(&self, parent_ino: u64, name: &str) -> Result<(), FsError> {
        let parent_num = u32::try_from(parent_ino).map_err(|_| FsError::BadFileDescriptor)?;
        let parent_inode = self
            .read_inode(parent_num)
            .map_err(|()| FsError::NotFound)?;

        if !parent_inode.is_dir() {
            return Err(FsError::NotSupported);
        }

        // Find the entry to get the inode number.
        let target_inode_num = self
            .find_entry(&parent_inode, name)
            .ok_or(FsError::NotFound)?;

        // Remove the directory entry.
        self.remove_dir_entry_from_disk(&parent_inode, name)
            .map_err(|()| FsError::IoError)?;

        // Read the target inode to free its blocks.
        let target_inode = self
            .read_inode(target_inode_num)
            .map_err(|()| FsError::IoError)?;

        // Free all data blocks of the target inode.
        self.free_inode_blocks(&target_inode)
            .map_err(|()| FsError::IoError)?;

        // Free the inode itself.
        self.free_inode(target_inode_num)
            .map_err(|()| FsError::IoError)?;

        Ok(())
    }

    fn symlink(&self, parent_ino: u64, name: &str, target: &str) -> Result<u64, FsError> {
        if name.is_empty() || name.len() > 255 {
            return Err(FsError::InvalidName);
        }

        let parent_num = u32::try_from(parent_ino).map_err(|_| FsError::BadFileDescriptor)?;
        let mut parent_inode = self
            .read_inode(parent_num)
            .map_err(|()| FsError::NotFound)?;

        if !parent_inode.is_dir() {
            return Err(FsError::NotSupported);
        }

        // Check if the name already exists.
        if self.find_entry(&parent_inode, name).is_some() {
            return Err(FsError::AlreadyExists);
        }

        // Allocate a new inode.
        let new_inode_num = self.alloc_inode().ok_or(FsError::NoSpace)?;

        let target_bytes = target.as_bytes();
        // Maximum fast symlink size: 15 u32 block pointers * 4 bytes = 60 bytes.
        const FAST_SYMLINK_MAX: usize = DIRECT_BLOCKS * core::mem::size_of::<u32>();

        let new_inode = if target_bytes.len() <= FAST_SYMLINK_MAX {
            // Fast symlink: target stored directly in i_block array.
            let mut block = [0u32; 15];
            // SAFETY: u32 and u8 arrays have a defined layout; we copy target
            // bytes into the block pointer array which is treated as raw bytes.
            let block_bytes: &mut [u8; 60] =
                unsafe { &mut *core::ptr::addr_of_mut!(block).cast::<[u8; 60]>() };
            block_bytes[..target_bytes.len()].copy_from_slice(target_bytes);

            Inode {
                mode: S_IFLNK | 0o777,
                uid: 0,
                size_low: target_bytes.len() as u32,
                atime: 0,
                ctime: 0,
                mtime: 0,
                dtime: 0,
                gid: 0,
                nlink: 1,
                blocks: 0, // No disk blocks allocated for fast symlink.
                block,
                size_high: 0,
            }
        } else {
            // Slow symlink: allocate a data block and write target there.
            let mut inode = Inode {
                mode: S_IFLNK | 0o777,
                uid: 0,
                size_low: 0,
                atime: 0,
                ctime: 0,
                mtime: 0,
                dtime: 0,
                gid: 0,
                nlink: 1,
                blocks: 0,
                block: [0u32; 15],
                size_high: 0,
            };
            self.write_inode(new_inode_num, &inode)
                .map_err(|()| FsError::IoError)?;

            // Write the target path as file data.
            self.write_inode_data(&mut inode, new_inode_num, 0, target_bytes)
                .map_err(|()| FsError::IoError)?;
            return self
                .add_dir_entry(
                    &mut parent_inode,
                    parent_num,
                    new_inode_num,
                    name,
                    EXT2_FT_SYMLINK,
                )
                .map_err(|()| FsError::IoError)
                .map(|()| u64::from(new_inode_num));
        };

        self.write_inode(new_inode_num, &new_inode)
            .map_err(|()| FsError::IoError)?;

        // Add directory entry.
        self.add_dir_entry(
            &mut parent_inode,
            parent_num,
            new_inode_num,
            name,
            EXT2_FT_SYMLINK,
        )
        .map_err(|()| FsError::IoError)?;

        Ok(u64::from(new_inode_num))
    }

    fn readlink(&self, ino: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        let inode_num = u32::try_from(ino).map_err(|_| FsError::BadFileDescriptor)?;
        let inode = self.read_inode(inode_num).map_err(|()| FsError::NotFound)?;

        if !inode.is_symlink() {
            return Err(FsError::NotSupported);
        }

        let target_len = inode.size() as usize;
        const FAST_SYMLINK_MAX: usize = DIRECT_BLOCKS * core::mem::size_of::<u32>();

        if target_len <= FAST_SYMLINK_MAX && inode.blocks == 0 {
            // Fast symlink: target is stored in i_block array.
            // SAFETY: u32 and u8 arrays have compatible alignment for reading.
            let block_bytes: &[u8; 60] =
                unsafe { &*core::ptr::addr_of!(inode.block).cast::<[u8; 60]>() };
            let to_copy = buf.len().min(target_len);
            buf[..to_copy].copy_from_slice(&block_bytes[..to_copy]);
            Ok(to_copy)
        } else {
            // Slow symlink: target is stored as regular file data.
            self.read_inode_data(&inode, 0, buf)
                .map_err(|()| FsError::IoError)
        }
    }

    fn mkdir(&self, parent_ino: u64, name: &str) -> Result<u64, FsError> {
        if name.is_empty() || name.len() > 255 {
            return Err(FsError::InvalidName);
        }

        let parent_num = u32::try_from(parent_ino).map_err(|_| FsError::BadFileDescriptor)?;
        let mut parent_inode = self
            .read_inode(parent_num)
            .map_err(|()| FsError::NotFound)?;

        if !parent_inode.is_dir() {
            return Err(FsError::NotADirectory);
        }

        // Check if the name already exists.
        if self.find_entry(&parent_inode, name).is_some() {
            return Err(FsError::AlreadyExists);
        }

        // Allocate a new inode for the directory.
        let new_inode_num = self.alloc_inode().ok_or(FsError::NoSpace)?;

        // Allocate a data block for the directory's . and .. entries.
        let dir_block = self.alloc_block().ok_or(FsError::NoSpace)?;

        // Initialize the new inode as a directory.
        let new_inode = Inode {
            mode: S_IFDIR | 0o755,
            uid: 0,
            size_low: BLOCK_SIZE,
            atime: 0,
            ctime: 0,
            mtime: 0,
            dtime: 0,
            gid: 0,
            nlink: 2, // . and parent link
            blocks: SECTORS_PER_BLOCK,
            block: {
                let mut b = [0u32; 15];
                b[0] = dir_block;
                b
            },
            size_high: 0,
        };
        self.write_inode(new_inode_num, &new_inode)
            .map_err(|()| FsError::IoError)?;

        // Build the directory block with "." and ".." entries.
        let mut block_data = vec![0u8; BLOCK_SIZE as usize];

        // "." entry: points to self.
        let dot_rec_len = ((8 + 1 + 3) & !3) as u16; // 12 bytes for "."
        write_u32(&mut block_data, 0, new_inode_num);
        write_u16_bytes(&mut block_data, 4, dot_rec_len);
        block_data[6] = 1; // name_len
        block_data[7] = EXT2_FT_DIR;
        block_data[8] = b'.';

        // ".." entry: occupies the rest of the block.
        let dotdot_offset = dot_rec_len as usize;
        let dotdot_rec_len = BLOCK_SIZE as u16 - dot_rec_len;
        write_u32(&mut block_data, dotdot_offset, parent_num);
        write_u16_bytes(&mut block_data, dotdot_offset + 4, dotdot_rec_len);
        block_data[dotdot_offset + 6] = 2; // name_len
        block_data[dotdot_offset + 7] = EXT2_FT_DIR;
        block_data[dotdot_offset + 8] = b'.';
        block_data[dotdot_offset + 9] = b'.';

        self.write_block(dir_block, &block_data)
            .map_err(|()| FsError::IoError)?;

        // Increment parent's link count (for ".." in the new directory).
        parent_inode.nlink += 1;
        self.write_inode(parent_num, &parent_inode)
            .map_err(|()| FsError::IoError)?;

        // Re-read parent after write to pass to add_dir_entry.
        parent_inode = self.read_inode(parent_num).map_err(|()| FsError::IoError)?;

        // Add the directory entry in the parent.
        self.add_dir_entry(
            &mut parent_inode,
            parent_num,
            new_inode_num,
            name,
            EXT2_FT_DIR,
        )
        .map_err(|()| FsError::IoError)?;

        Ok(u64::from(new_inode_num))
    }

    fn rmdir(&self, parent_ino: u64, name: &str) -> Result<(), FsError> {
        let parent_num = u32::try_from(parent_ino).map_err(|_| FsError::BadFileDescriptor)?;
        let parent_inode = self
            .read_inode(parent_num)
            .map_err(|()| FsError::NotFound)?;

        if !parent_inode.is_dir() {
            return Err(FsError::NotADirectory);
        }

        // Find the entry.
        let target_inode_num = self
            .find_entry(&parent_inode, name)
            .ok_or(FsError::NotFound)?;

        // Read the target inode and verify it's a directory.
        let target_inode = self
            .read_inode(target_inode_num)
            .map_err(|()| FsError::IoError)?;

        if !target_inode.is_dir() {
            return Err(FsError::NotADirectory);
        }

        // Check that the directory is empty (only . and .. entries).
        let entries = self
            .read_dir_entries(&target_inode)
            .map_err(|()| FsError::IoError)?;
        let non_special = entries
            .iter()
            .filter(|e| e.name != b".".as_slice() && e.name != b"..".as_slice())
            .count();
        if non_special > 0 {
            return Err(FsError::IoError);
        }

        // Remove the directory entry from the parent.
        self.remove_dir_entry_from_disk(&parent_inode, name)
            .map_err(|()| FsError::IoError)?;

        // Free the directory's data blocks.
        self.free_inode_blocks(&target_inode)
            .map_err(|()| FsError::IoError)?;

        // Free the inode.
        self.free_inode(target_inode_num)
            .map_err(|()| FsError::IoError)?;

        // Decrement parent's link count (lost the ".." reference).
        let mut parent_inode = self.read_inode(parent_num).map_err(|()| FsError::IoError)?;
        parent_inode.nlink = parent_inode.nlink.saturating_sub(1);
        self.write_inode(parent_num, &parent_inode)
            .map_err(|()| FsError::IoError)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;

    use alloc::vec;

    use super::*;

    // ─────────────────── Constant tests ───────────────────

    #[test]
    fn ext2_magic_value() {
        assert_eq!(EXT2_MAGIC, 0xEF53);
    }

    #[test]
    fn block_size_value() {
        assert_eq!(BLOCK_SIZE, 1024);
    }

    #[test]
    fn sector_size_value() {
        assert_eq!(SECTOR_SIZE, 512);
    }

    #[test]
    fn sectors_per_block() {
        assert_eq!(SECTORS_PER_BLOCK, BLOCK_SIZE / SECTOR_SIZE);
        assert_eq!(SECTORS_PER_BLOCK, 2);
    }

    #[test]
    fn inode_size_value() {
        assert_eq!(INODE_SIZE, 128);
    }

    #[test]
    fn root_inode_number() {
        assert_eq!(ROOT_INODE, 2);
    }

    #[test]
    fn s_ifdir_mask() {
        assert_eq!(S_IFDIR, 0x4000);
    }

    #[test]
    fn s_ifreg_mask() {
        assert_eq!(S_IFREG, 0x8000);
    }

    #[test]
    fn direct_blocks_count() {
        assert_eq!(DIRECT_BLOCKS, 12);
    }

    #[test]
    fn ext2_ft_reg_file() {
        assert_eq!(EXT2_FT_REG_FILE, 1);
    }

    #[test]
    fn ext2_ft_dir() {
        assert_eq!(EXT2_FT_DIR, 2);
    }

    // ─────────────────── Byte helper tests ───────────────────

    #[test]
    fn read_u16_le() {
        let data = [0x34, 0x12];
        assert_eq!(super::read_u16(&data, 0), 0x1234);
    }

    #[test]
    fn read_u32_le() {
        let data = [0x78, 0x56, 0x34, 0x12];
        assert_eq!(super::read_u32(&data, 0), 0x12345678);
    }

    #[test]
    fn read_u16_at_offset() {
        let data = [0x00, 0x00, 0x34, 0x12];
        assert_eq!(super::read_u16(&data, 2), 0x1234);
    }

    #[test]
    fn read_u32_at_offset() {
        let data = [0x00, 0x00, 0x78, 0x56, 0x34, 0x12];
        assert_eq!(super::read_u32(&data, 2), 0x12345678);
    }

    #[test]
    fn write_u32_le() {
        let mut data = [0u8; 4];
        super::write_u32(&mut data, 0, 0x12345678);
        assert_eq!(data, [0x78, 0x56, 0x34, 0x12]);
    }

    #[test]
    fn write_u16_bytes_le() {
        let mut data = [0u8; 2];
        super::write_u16_bytes(&mut data, 0, 0x1234);
        assert_eq!(data, [0x34, 0x12]);
    }

    #[test]
    fn write_u32_at_offset() {
        let mut data = [0u8; 8];
        super::write_u32(&mut data, 4, 0xDEADBEEF);
        assert_eq!(super::read_u32(&data, 4), 0xDEADBEEF);
    }

    #[test]
    fn write_read_roundtrip_u16() {
        let mut data = [0u8; 16];
        super::write_u16_bytes(&mut data, 6, 0xABCD);
        assert_eq!(super::read_u16(&data, 6), 0xABCD);
    }

    #[test]
    fn write_read_roundtrip_u32() {
        let mut data = [0u8; 16];
        super::write_u32(&mut data, 8, 0xCAFEBABE);
        assert_eq!(super::read_u32(&data, 8), 0xCAFEBABE);
    }

    // ─────────────────── Superblock tests ───────────────────

    #[test]
    fn superblock_rejects_invalid_magic() {
        let mut data = [0u8; 1024];
        // Write an invalid magic at offset 56.
        super::write_u16_bytes(&mut data, 56, 0x0000);
        assert!(Superblock::from_bytes(&data).is_none());
    }

    #[test]
    fn superblock_rejects_wrong_magic() {
        let mut data = [0u8; 1024];
        super::write_u16_bytes(&mut data, 56, 0x1234);
        assert!(Superblock::from_bytes(&data).is_none());
    }

    #[test]
    fn superblock_parses_valid_magic() {
        let mut data = [0u8; 1024];
        // Write the correct magic.
        super::write_u16_bytes(&mut data, 56, EXT2_MAGIC);
        // Write some field values to verify parsing.
        super::write_u32(&mut data, 0, 1000); // inodes_count
        super::write_u32(&mut data, 4, 2000); // blocks_count
        super::write_u32(&mut data, 24, 0); // log_block_size (0 = 1024 bytes)
        super::write_u32(&mut data, 32, 8192); // blocks_per_group
        super::write_u32(&mut data, 40, 1000); // inodes_per_group

        let sb = Superblock::from_bytes(&data).expect("should parse valid superblock");
        assert_eq!(sb.inodes_count, 1000);
        assert_eq!(sb.blocks_count, 2000);
        assert_eq!(sb.log_block_size, 0);
        assert_eq!(sb.blocks_per_group, 8192);
        assert_eq!(sb.inodes_per_group, 1000);
        assert_eq!(sb.magic, EXT2_MAGIC);
    }

    #[test]
    fn superblock_rev_level_fields() {
        let mut data = [0u8; 1024];
        super::write_u16_bytes(&mut data, 56, EXT2_MAGIC);
        super::write_u32(&mut data, 76, 1); // rev_level
        super::write_u16_bytes(&mut data, 88, 128); // inode_size at offset 88
        super::write_u32(&mut data, 84, 11); // first_ino

        let sb = Superblock::from_bytes(&data).unwrap();
        assert_eq!(sb.rev_level, 1);
        assert_eq!(sb.inode_size, 128);
        assert_eq!(sb.first_ino, 11);
    }

    // ─────────────────── BlockGroupDescriptor tests ───────────────────

    #[test]
    fn block_group_descriptor_from_bytes() {
        let mut data = vec![0u8; 32];
        super::write_u32(&mut data, 0, 5); // block_bitmap
        super::write_u32(&mut data, 4, 6); // inode_bitmap
        super::write_u32(&mut data, 8, 7); // inode_table
        super::write_u16_bytes(&mut data, 12, 100); // free_blocks_count
        super::write_u16_bytes(&mut data, 14, 50); // free_inodes_count
        super::write_u16_bytes(&mut data, 16, 3); // used_dirs_count

        let desc = BlockGroupDescriptor::from_bytes(&data);
        assert_eq!(desc.block_bitmap, 5);
        assert_eq!(desc.inode_bitmap, 6);
        assert_eq!(desc.inode_table, 7);
        assert_eq!(desc.free_blocks_count, 100);
        assert_eq!(desc.free_inodes_count, 50);
        assert_eq!(desc.used_dirs_count, 3);
    }

    // ─────────────────── Inode tests ───────────────────

    #[test]
    fn inode_from_bytes() {
        let mut data = [0u8; 128];
        // mode at offset 0
        super::write_u16_bytes(&mut data, 0, S_IFREG | 0o644);
        // uid at offset 2
        super::write_u16_bytes(&mut data, 2, 1000);
        // size_low at offset 4
        super::write_u32(&mut data, 4, 4096);
        // nlink at offset 26
        super::write_u16_bytes(&mut data, 26, 1);
        // blocks at offset 28
        super::write_u32(&mut data, 28, 8);
        // block[0] at offset 40
        super::write_u32(&mut data, 40, 100);
        // block[1] at offset 44
        super::write_u32(&mut data, 44, 101);

        let inode = Inode::from_bytes(&data);
        assert_eq!(inode.mode, S_IFREG | 0o644);
        assert_eq!(inode.uid, 1000);
        assert_eq!(inode.size_low, 4096);
        assert_eq!(inode.nlink, 1);
        assert_eq!(inode.blocks, 8);
        assert_eq!(inode.block[0], 100);
        assert_eq!(inode.block[1], 101);
    }

    #[test]
    fn inode_is_dir_true() {
        let inode = Inode {
            mode: S_IFDIR | 0o755,
            uid: 0,
            size_low: 0,
            atime: 0,
            ctime: 0,
            mtime: 0,
            dtime: 0,
            gid: 0,
            nlink: 2,
            blocks: 0,
            block: [0u32; 15],
            size_high: 0,
        };
        assert!(inode.is_dir());
        assert!(!inode.is_reg());
    }

    #[test]
    fn inode_is_reg_true() {
        let inode = Inode {
            mode: S_IFREG | 0o644,
            uid: 0,
            size_low: 100,
            atime: 0,
            ctime: 0,
            mtime: 0,
            dtime: 0,
            gid: 0,
            nlink: 1,
            blocks: 1,
            block: [0u32; 15],
            size_high: 0,
        };
        assert!(!inode.is_dir());
        assert!(inode.is_reg());
    }

    #[test]
    fn inode_size_for_dir() {
        // Directory size uses only size_low (no size_high).
        let inode = Inode {
            mode: S_IFDIR,
            uid: 0,
            size_low: 1024,
            atime: 0,
            ctime: 0,
            mtime: 0,
            dtime: 0,
            gid: 0,
            nlink: 2,
            blocks: 2,
            block: [0u32; 15],
            size_high: 0x1234_5678, // Should be ignored for dirs.
        };
        assert_eq!(inode.size(), 1024);
    }

    #[test]
    fn inode_size_for_reg_file() {
        // Regular file size combines size_high and size_low.
        let inode = Inode {
            mode: S_IFREG,
            uid: 0,
            size_low: 0x0000_1000,
            atime: 0,
            ctime: 0,
            mtime: 0,
            dtime: 0,
            gid: 0,
            nlink: 1,
            blocks: 8,
            block: [0u32; 15],
            size_high: 0x0000_0001,
        };
        assert_eq!(inode.size(), 0x0000_0001_0000_1000);
    }

    #[test]
    fn inode_to_bytes_roundtrip() {
        let inode = Inode {
            mode: S_IFREG | 0o644,
            uid: 500,
            size_low: 8192,
            atime: 1000,
            ctime: 2000,
            mtime: 3000,
            dtime: 0,
            gid: 100,
            nlink: 1,
            blocks: 16,
            block: {
                let mut b = [0u32; 15];
                b[0] = 10;
                b[1] = 11;
                b[12] = 100; // indirect
                b
            },
            size_high: 0,
        };

        let bytes = inode.to_bytes();
        let parsed = Inode::from_bytes(&bytes);

        assert_eq!(parsed.mode, inode.mode);
        assert_eq!(parsed.uid, inode.uid);
        assert_eq!(parsed.size_low, inode.size_low);
        assert_eq!(parsed.atime, inode.atime);
        assert_eq!(parsed.ctime, inode.ctime);
        assert_eq!(parsed.mtime, inode.mtime);
        assert_eq!(parsed.gid, inode.gid);
        assert_eq!(parsed.nlink, inode.nlink);
        assert_eq!(parsed.blocks, inode.blocks);
        assert_eq!(parsed.block[0], 10);
        assert_eq!(parsed.block[1], 11);
        assert_eq!(parsed.block[12], 100);
        assert_eq!(parsed.size_high, inode.size_high);
    }

    // ─────────────────── Block pointer calculation tests ───────────────────

    #[test]
    fn direct_block_range() {
        // Direct blocks cover indices 0..11.
        assert_eq!(DIRECT_BLOCKS, 12);
    }

    #[test]
    fn indirect_block_start_index() {
        // Single indirect starts at index 12.
        let entries_per_block: u32 = BLOCK_SIZE / 4; // 1024 / 4 = 256
        let indirect_start = DIRECT_BLOCKS as u32;
        assert_eq!(indirect_start, 12);
        // Single indirect covers 12..268 (12 + 256).
        let indirect_end = indirect_start + entries_per_block;
        assert_eq!(indirect_end, 268);
    }

    #[test]
    fn double_indirect_range() {
        let entries_per_block: u32 = BLOCK_SIZE / 4; // 256
        let double_start = DIRECT_BLOCKS as u32 + entries_per_block; // 268
        let double_end = double_start + entries_per_block * entries_per_block; // 268 + 65536
        assert_eq!(double_start, 268);
        assert_eq!(double_end, 65804);
    }

    #[test]
    fn triple_indirect_range() {
        let entries_per_block: u32 = BLOCK_SIZE / 4;
        let triple_start =
            DIRECT_BLOCKS as u32 + entries_per_block + entries_per_block * entries_per_block;
        assert_eq!(triple_start, 65804);
    }

    #[test]
    fn entries_per_block_for_1024_byte_blocks() {
        let entries = BLOCK_SIZE / 4;
        assert_eq!(entries, 256);
    }

    // ─────────────────── Indirect block index tests ───────────────────

    #[test]
    fn single_indirect_index_calculation() {
        // For logical block 12 (first indirect), the index within the
        // indirect block should be 0.
        let entries_per_block: u32 = BLOCK_SIZE / 4;
        let logical: u32 = 12;
        assert!(logical >= DIRECT_BLOCKS as u32);
        assert!(logical < DIRECT_BLOCKS as u32 + entries_per_block);
        let index = logical - DIRECT_BLOCKS as u32;
        assert_eq!(index, 0);
    }

    #[test]
    fn single_indirect_last_index() {
        let entries_per_block: u32 = BLOCK_SIZE / 4;
        let logical = DIRECT_BLOCKS as u32 + entries_per_block - 1; // 267
        let index = logical - DIRECT_BLOCKS as u32;
        assert_eq!(index, entries_per_block - 1);
        assert_eq!(index, 255);
    }

    #[test]
    fn double_indirect_first_level_index() {
        let entries_per_block: u32 = BLOCK_SIZE / 4;
        let base = DIRECT_BLOCKS as u32 + entries_per_block; // 268
        let logical = base; // First double-indirect block.
        let index = logical - base;
        let first = index / entries_per_block;
        let second = index % entries_per_block;
        assert_eq!(first, 0);
        assert_eq!(second, 0);
    }

    #[test]
    fn double_indirect_second_level_index() {
        let entries_per_block: u32 = BLOCK_SIZE / 4;
        let base = DIRECT_BLOCKS as u32 + entries_per_block;
        let logical = base + 257; // Second slot in second indirect block.
        let index = logical - base;
        let first = index / entries_per_block;
        let second = index % entries_per_block;
        assert_eq!(first, 1);
        assert_eq!(second, 1);
    }

    #[test]
    fn triple_indirect_first_level_index() {
        let entries_per_block: u32 = BLOCK_SIZE / 4;
        let base = DIRECT_BLOCKS as u32 + entries_per_block + entries_per_block * entries_per_block;
        let logical = base;
        let index = logical - base;
        let first = index / (entries_per_block * entries_per_block);
        assert_eq!(first, 0);
    }

    // ─────────────────── Directory entry tests ───────────────────

    #[test]
    fn dir_entry_raw_fields() {
        let entry = DirEntryRaw {
            inode: 42,
            rec_len: 12,
            name_len: 4,
            file_type: EXT2_FT_REG_FILE,
            name: b"test".to_vec(),
        };
        assert_eq!(entry.inode, 42);
        assert_eq!(entry.rec_len, 12);
        assert_eq!(entry.name_len, 4);
        assert_eq!(entry.file_type, EXT2_FT_REG_FILE);
        assert_eq!(entry.name, b"test");
    }

    #[test]
    fn dir_entry_name_matching() {
        let entry = DirEntryRaw {
            inode: 10,
            rec_len: 16,
            name_len: 5,
            file_type: EXT2_FT_DIR,
            name: b"hello".to_vec(),
        };
        assert_eq!(entry.name, b"hello");
        assert_ne!(entry.name, b"world");
    }

    // ─────────────────── Inode mode bit tests ───────────────────

    #[test]
    fn mode_dir_with_permissions() {
        let mode = S_IFDIR | 0o755;
        assert_eq!(mode & 0xF000, S_IFDIR);
        assert_eq!(mode & 0o777, 0o755);
    }

    #[test]
    fn mode_reg_with_permissions() {
        let mode = S_IFREG | 0o644;
        assert_eq!(mode & 0xF000, S_IFREG);
        assert_eq!(mode & 0o777, 0o644);
    }

    #[test]
    fn mode_type_mask_extraction() {
        // The type is in bits 15..12 (mask 0xF000).
        assert_eq!(S_IFDIR & 0xF000, S_IFDIR);
        assert_eq!(S_IFREG & 0xF000, S_IFREG);
    }

    // ─────────────────── Ext2Fs struct tests ───────────────────

    #[test]
    fn open_rejects_invalid_device() {
        // Device 99 doesn't exist in the block registry.
        assert!(Ext2Fs::open(99).is_err());
    }

    // ─────────────────── Record length alignment ───────────────────

    #[test]
    fn dir_entry_size_alignment() {
        // Directory entry sizes must be 4-byte aligned.
        // The formula is: (8 + name_len + 3) & !3
        let name_len = 5usize;
        let needed = ((8 + name_len + 3) & !3) as u16;
        assert_eq!(needed, 16); // 8 + 5 = 13, rounded up to 16.

        let name_len = 4;
        let needed = ((8 + name_len + 3) & !3) as u16;
        assert_eq!(needed, 12); // 8 + 4 = 12, already aligned.

        let name_len = 1;
        let needed = ((8 + name_len + 3) & !3) as u16;
        assert_eq!(needed, 12); // 8 + 1 = 9, rounded to 12.
    }

    // ─────────────────── Symlink constants ───────────────────

    #[test]
    fn s_iflnk_mask() {
        assert_eq!(S_IFLNK, 0xA000);
    }

    #[test]
    fn ext2_ft_symlink_value() {
        assert_eq!(EXT2_FT_SYMLINK, 7);
    }

    #[test]
    fn inode_is_symlink_true() {
        let inode = Inode {
            mode: S_IFLNK | 0o777,
            uid: 0,
            size_low: 10,
            atime: 0,
            ctime: 0,
            mtime: 0,
            dtime: 0,
            gid: 0,
            nlink: 1,
            blocks: 0,
            block: [0u32; 15],
            size_high: 0,
        };
        assert!(inode.is_symlink());
        assert!(!inode.is_dir());
        assert!(!inode.is_reg());
    }

    #[test]
    fn fast_symlink_max_size() {
        // Fast symlink stores target in i_block[0..DIRECT_BLOCKS] as bytes.
        const FAST_SYMLINK_MAX: usize = DIRECT_BLOCKS * core::mem::size_of::<u32>();
        assert_eq!(FAST_SYMLINK_MAX, 48);
    }

    #[test]
    fn symlink_inode_mode_roundtrip() {
        let inode = Inode {
            mode: S_IFLNK | 0o777,
            uid: 0,
            size_low: 5,
            atime: 0,
            ctime: 0,
            mtime: 0,
            dtime: 0,
            gid: 0,
            nlink: 1,
            blocks: 0,
            block: [0u32; 15],
            size_high: 0,
        };
        let bytes = inode.to_bytes();
        let parsed = Inode::from_bytes(&bytes);
        assert_eq!(parsed.mode, S_IFLNK | 0o777);
        assert!(parsed.is_symlink());
    }

    #[test]
    fn symlink_fast_target_in_block_array() {
        let target = b"/usr/bin/env";
        let mut block = [0u32; 15];
        let block_bytes: &mut [u8; 60] = unsafe { core::mem::transmute(&mut block) };
        block_bytes[..target.len()].copy_from_slice(target);

        // Read it back.
        let read_bytes: &[u8; 60] = unsafe { core::mem::transmute(&block) };
        assert_eq!(&read_bytes[..target.len()], target);
    }
}
