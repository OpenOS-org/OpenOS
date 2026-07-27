//! Device filesystem (devfs) — virtual filesystem for device nodes.
//!
//! Provides virtual files that map to hardware devices or special sinks:
//!
//! - `/dev/null` — read returns EOF, write discards data
//! - `/dev/zero` — read returns zero bytes, write discards data
//! - `/dev/serial` — read/write backed by UART 0x3F8 (COM1)
//! - `/dev/console` — alias for serial
//! - `/dev/random` — pseudo-random bytes (LCG)
//! - `/dev/urandom` — same as random (non-blocking)
//!
//! ## Inode scheme
//!
//! Synthetic inode numbers identify each device:
//! - `1` = root directory (`/dev`)
//! - `2` = `null`
//! - `3` = `zero`
//! - `4` = `serial`
//! - `5` = `console`
//! - `6` = `random`
//! - `7` = `urandom`

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use super::vfs::{DirEntry, FileSystem, FsError, InodeMeta, OpenFlags};

// ---------------------------------------------------------------------------
// Inode number constants
// ---------------------------------------------------------------------------

/// Inode number for the devfs root directory.
const ROOT_INO: u64 = 1;

/// Inode number for `/dev/null`.
const NULL_INO: u64 = 2;

/// Inode number for `/dev/zero`.
const ZERO_INO: u64 = 3;

/// Inode number for `/dev/serial`.
const SERIAL_INO: u64 = 4;

/// Inode number for `/dev/console`.
const CONSOLE_INO: u64 = 5;

/// Inode number for `/dev/random`.
const RANDOM_INO: u64 = 6;

/// Inode number for `/dev/urandom`.
const URANDOM_INO: u64 = 7;

// ---------------------------------------------------------------------------
// Pseudo-random number generator (LCG)
// ---------------------------------------------------------------------------

/// Linear congruential generator state for `/dev/random` and `/dev/urandom`.
///
/// Uses the Numerical Recipes parameters: `a = 1664525`, `c = 1013904223`.
static PRNG_STATE: AtomicU64 = AtomicU64::new(1);

/// Generate the next pseudo-random `u64` from the LCG.
fn next_random() -> u64 {
    let old = PRNG_STATE.load(Ordering::Relaxed);
    // LCG: next = old * a + c (mod 2^64)
    let next = old.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    PRNG_STATE.store(next, Ordering::Relaxed);
    next
}

// ---------------------------------------------------------------------------
// DevFs implementation
// ---------------------------------------------------------------------------

/// Device filesystem — implements `FileSystem` for device nodes.
pub struct DevFs;

impl DevFs {
    /// Parse a path relative to `/dev/` and return the synthetic inode number.
    fn resolve_path(path: &str) -> Option<u64> {
        let path = path.trim_start_matches('/');
        if path.is_empty() || path == "." {
            return Some(ROOT_INO);
        }
        match path {
            "null" => Some(NULL_INO),
            "zero" => Some(ZERO_INO),
            "serial" => Some(SERIAL_INO),
            "console" => Some(CONSOLE_INO),
            "random" => Some(RANDOM_INO),
            "urandom" => Some(URANDOM_INO),
            _ => None,
        }
    }

    /// Fill a buffer with pseudo-random bytes.
    fn fill_random(buf: &mut [u8]) {
        let mut remaining = buf;
        while !remaining.is_empty() {
            let val = next_random();
            let bytes = val.to_ne_bytes();
            let chunk = remaining.len().min(8);
            remaining[..chunk].copy_from_slice(&bytes[..chunk]);
            remaining = &mut remaining[chunk..];
        }
    }

    /// Write data to the serial port (COM1).
    fn write_serial(data: &[u8]) {
        let mut serial = crate::drivers::serial::SERIAL1.lock();
        for &byte in data {
            serial.send(byte);
        }
    }

    /// Read bytes from the serial port (COM1) into a buffer.
    ///
    /// Returns the number of bytes actually read. Non-blocking: returns 0
    /// if no data is available.
    fn read_serial(buf: &mut [u8]) -> usize {
        let mut serial = crate::drivers::serial::SERIAL1.lock();
        let mut count = 0;
        for byte in buf.iter_mut() {
            match serial.try_receive() {
                Ok(b) => {
                    *byte = b;
                    count += 1;
                }
                Err(_) => break,
            }
        }
        count
    }
}

impl FileSystem for DevFs {
    fn open(&self, path: &str, _flags: OpenFlags) -> Result<u64, FsError> {
        Self::resolve_path(path).ok_or(FsError::NotFound)
    }

    fn close(&self, _ino: u64) -> Result<(), FsError> {
        Ok(())
    }

    fn read(&self, ino: u64, _offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        match ino {
            ROOT_INO => Err(FsError::NotSupported),
            NULL_INO => Ok(0), // Always EOF.
            ZERO_INO => {
                // Fill buffer with zeros.
                for byte in buf.iter_mut() {
                    *byte = 0;
                }
                Ok(buf.len())
            }
            SERIAL_INO | CONSOLE_INO => Ok(Self::read_serial(buf)),
            RANDOM_INO | URANDOM_INO => {
                Self::fill_random(buf);
                Ok(buf.len())
            }
            _ => Err(FsError::NotFound),
        }
    }

    fn write(&self, ino: u64, _offset: u64, data: &[u8]) -> Result<usize, FsError> {
        match ino {
            ROOT_INO => Err(FsError::NotSupported),
            NULL_INO | ZERO_INO | RANDOM_INO | URANDOM_INO => {
                // Discard all writes (sink).
                Ok(data.len())
            }
            SERIAL_INO | CONSOLE_INO => {
                Self::write_serial(data);
                Ok(data.len())
            }
            _ => Err(FsError::NotFound),
        }
    }

    fn stat(&self, ino: u64) -> Result<InodeMeta, FsError> {
        match ino {
            ROOT_INO => Ok(InodeMeta {
                ino: ROOT_INO,
                is_dir: true,
                is_symlink: false,
                is_fifo: false,
                size: 0,
                nlink: 1,
            }),
            NULL_INO | ZERO_INO | SERIAL_INO | CONSOLE_INO | RANDOM_INO | URANDOM_INO => {
                Ok(InodeMeta {
                    ino,
                    is_dir: false,
                    is_symlink: false,
                    is_fifo: false,
                    size: 0,
                    nlink: 1,
                })
            }
            _ => Err(FsError::NotFound),
        }
    }

    fn readdir(&self, dir_ino: u64) -> Result<Vec<DirEntry>, FsError> {
        if dir_ino != ROOT_INO {
            return Err(FsError::NotFound);
        }

        let mut entries = Vec::new();
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

        // Static device list.
        const DEVICES: &[(u64, &str)] = &[
            (NULL_INO, "null"),
            (ZERO_INO, "zero"),
            (SERIAL_INO, "serial"),
            (CONSOLE_INO, "console"),
            (RANDOM_INO, "random"),
            (URANDOM_INO, "urandom"),
        ];

        for &(ino, name) in DEVICES {
            entries.push(DirEntry {
                name: String::from(name),
                ino,
                is_dir: false,
            });
        }

        Ok(entries)
    }

    fn create(&self, _parent_ino: u64, _name: &str) -> Result<u64, FsError> {
        Err(FsError::PermissionDenied)
    }

    fn unlink(&self, _parent_ino: u64, _name: &str) -> Result<(), FsError> {
        Err(FsError::PermissionDenied)
    }

    fn symlink(&self, _parent_ino: u64, _name: &str, _target: &str) -> Result<u64, FsError> {
        Err(FsError::NotSupported)
    }

    fn readlink(&self, _ino: u64, _buf: &mut [u8]) -> Result<usize, FsError> {
        Err(FsError::NotSupported)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_root() {
        assert_eq!(DevFs::resolve_path(""), Some(ROOT_INO));
        assert_eq!(DevFs::resolve_path("."), Some(ROOT_INO));
    }

    #[test]
    fn test_resolve_devices() {
        assert_eq!(DevFs::resolve_path("null"), Some(NULL_INO));
        assert_eq!(DevFs::resolve_path("zero"), Some(ZERO_INO));
        assert_eq!(DevFs::resolve_path("serial"), Some(SERIAL_INO));
        assert_eq!(DevFs::resolve_path("console"), Some(CONSOLE_INO));
        assert_eq!(DevFs::resolve_path("random"), Some(RANDOM_INO));
        assert_eq!(DevFs::resolve_path("urandom"), Some(URANDOM_INO));
    }

    #[test]
    fn test_resolve_unknown() {
        assert_eq!(DevFs::resolve_path("nonexistent"), None);
        assert_eq!(DevFs::resolve_path("foo/bar"), None);
    }

    #[test]
    fn test_null_read_eof() {
        let fs = DevFs;
        let mut buf = [0xFFu8; 16];
        let n = fs.read(NULL_INO, 0, &mut buf).unwrap();
        assert_eq!(n, 0);
        // Buffer should be unchanged.
        assert!(buf.iter().all(|&b| b == 0xFF));
    }

    #[test]
    fn test_null_write_succeeds() {
        let fs = DevFs;
        let written = fs.write(NULL_INO, 0, b"discard this").unwrap();
        assert_eq!(written, 12);
    }

    #[test]
    fn test_zero_read_fills_zeros() {
        let fs = DevFs;
        let mut buf = [0xFFu8; 32];
        let n = fs.read(ZERO_INO, 0, &mut buf).unwrap();
        assert_eq!(n, 32);
        assert!(buf.iter().all(|&b| b == 0));
    }

    #[test]
    fn test_random_read_fills_buffer() {
        let fs = DevFs;
        let mut buf = [0u8; 64];
        let n = fs.read(RANDOM_INO, 0, &mut buf).unwrap();
        assert_eq!(n, 64);
        // Extremely unlikely to be all zeros from an LCG.
        assert!(buf.iter().any(|&b| b != 0));
    }

    #[test]
    fn test_urandom_read_fills_buffer() {
        let fs = DevFs;
        let mut buf = [0u8; 64];
        let n = fs.read(URANDOM_INO, 0, &mut buf).unwrap();
        assert_eq!(n, 64);
        assert!(buf.iter().any(|&b| b != 0));
    }

    #[test]
    fn test_random_not_constant() {
        let fs = DevFs;
        let mut buf1 = [0u8; 32];
        let mut buf2 = [0u8; 32];
        fs.read(RANDOM_INO, 0, &mut buf1).unwrap();
        fs.read(RANDOM_INO, 0, &mut buf2).unwrap();
        // Two reads should produce different data (with overwhelming probability).
        assert_ne!(buf1, buf2);
    }

    #[test]
    fn test_open_and_close() {
        let fs = DevFs;
        let ino = fs.open("null", OpenFlags::READ).unwrap();
        assert_eq!(ino, NULL_INO);
        fs.close(ino).unwrap();
    }

    #[test]
    fn test_open_not_found() {
        let fs = DevFs;
        assert_eq!(
            fs.open("nonexistent", OpenFlags::READ),
            Err(FsError::NotFound)
        );
    }

    #[test]
    fn test_write_denied_for_non_device() {
        let fs = DevFs;
        assert_eq!(fs.write(9999, 0, b"data"), Err(FsError::NotFound));
    }

    #[test]
    fn test_read_denied_for_non_device() {
        let fs = DevFs;
        let mut buf = [0u8; 8];
        assert_eq!(fs.read(9999, 0, &mut buf), Err(FsError::NotFound));
    }

    #[test]
    fn test_stat_root() {
        let fs = DevFs;
        let meta = fs.stat(ROOT_INO).unwrap();
        assert!(meta.is_dir);
        assert_eq!(meta.ino, ROOT_INO);
    }

    #[test]
    fn test_stat_devices() {
        let fs = DevFs;
        for ino in &[
            NULL_INO,
            ZERO_INO,
            SERIAL_INO,
            CONSOLE_INO,
            RANDOM_INO,
            URANDOM_INO,
        ] {
            let meta = fs.stat(*ino).unwrap();
            assert!(!meta.is_dir);
            assert_eq!(meta.ino, *ino);
        }
    }

    #[test]
    fn test_stat_unknown() {
        let fs = DevFs;
        assert_eq!(fs.stat(9999), Err(FsError::NotFound));
    }

    #[test]
    fn test_readdir_root() {
        let fs = DevFs;
        let entries = fs.readdir(ROOT_INO).unwrap();
        assert!(entries.iter().any(|e| e.name == "."));
        assert!(entries.iter().any(|e| e.name == ".."));
        assert!(entries.iter().any(|e| e.name == "null"));
        assert!(entries.iter().any(|e| e.name == "zero"));
        assert!(entries.iter().any(|e| e.name == "serial"));
        assert!(entries.iter().any(|e| e.name == "console"));
        assert!(entries.iter().any(|e| e.name == "random"));
        assert!(entries.iter().any(|e| e.name == "urandom"));
    }

    #[test]
    fn test_readdir_non_root() {
        let fs = DevFs;
        assert_eq!(fs.readdir(NULL_INO), Err(FsError::NotFound));
    }

    #[test]
    fn test_create_denied() {
        let fs = DevFs;
        assert_eq!(fs.create(ROOT_INO, "bad"), Err(FsError::PermissionDenied));
    }

    #[test]
    fn test_unlink_denied() {
        let fs = DevFs;
        assert_eq!(fs.unlink(ROOT_INO, "bad"), Err(FsError::PermissionDenied));
    }

    #[test]
    fn test_fill_random_length() {
        let mut buf = [0u8; 17]; // Non-multiple of 8.
        DevFs::fill_random(&mut buf);
        assert!(buf.iter().any(|&b| b != 0));
    }

    #[test]
    fn test_zero_write_succeeds() {
        let fs = DevFs;
        let written = fs.write(ZERO_INO, 0, b"data").unwrap();
        assert_eq!(written, 4);
    }

    #[test]
    fn test_random_write_succeeds() {
        let fs = DevFs;
        let written = fs.write(RANDOM_INO, 0, b"data").unwrap();
        assert_eq!(written, 4);
    }

    // ─── Additional tests ───

    #[test]
    fn test_resolve_with_leading_slash() {
        assert_eq!(DevFs::resolve_path("/null"), Some(NULL_INO));
        assert_eq!(DevFs::resolve_path("/serial"), Some(SERIAL_INO));
        assert_eq!(DevFs::resolve_path("/random"), Some(RANDOM_INO));
    }

    #[test]
    fn test_zero_read_small_buf() {
        let fs = DevFs;
        let mut buf = [0xFFu8; 1];
        let n = fs.read(ZERO_INO, 0, &mut buf).unwrap();
        assert_eq!(n, 1);
        assert_eq!(buf[0], 0);
    }

    #[test]
    fn test_zero_read_empty_buf() {
        let fs = DevFs;
        let mut buf: [u8; 0] = [];
        let n = fs.read(ZERO_INO, 0, &mut buf).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn test_null_write_empty() {
        let fs = DevFs;
        let written = fs.write(NULL_INO, 0, b"").unwrap();
        assert_eq!(written, 0);
    }

    #[test]
    fn test_urandom_write_succeeds() {
        let fs = DevFs;
        let written = fs.write(URANDOM_INO, 0, b"data").unwrap();
        assert_eq!(written, 4);
    }

    #[test]
    fn test_write_to_root_fails() {
        let fs = DevFs;
        assert_eq!(fs.write(ROOT_INO, 0, b"data"), Err(FsError::NotSupported));
    }

    #[test]
    fn test_read_root_fails() {
        let fs = DevFs;
        let mut buf = [0u8; 8];
        assert_eq!(fs.read(ROOT_INO, 0, &mut buf), Err(FsError::NotSupported));
    }

    #[test]
    fn test_prng_deterministic_sequence() {
        // Reset the LCG state and verify deterministic output.
        PRNG_STATE.store(42, core::sync::atomic::Ordering::Relaxed);
        // Read exactly 8 bytes (one u64).
        let val1 = next_random();
        // Reset and read again.
        PRNG_STATE.store(42, core::sync::atomic::Ordering::Relaxed);
        let val2 = next_random();
        assert_eq!(val1, val2);
    }

    #[test]
    fn test_fill_random_various_lengths() {
        // Test fill_random with buffers of different sizes.
        for len in &[0usize, 1, 3, 7, 8, 9, 16, 32] {
            let mut buf = vec![0u8; *len];
            DevFs::fill_random(&mut buf);
            if *len > 0 {
                assert!(buf.iter().any(|&b| b != 0), "all zeros at len={}", len);
            }
        }
    }

    #[test]
    fn test_stat_null() {
        let fs = DevFs;
        let meta = fs.stat(NULL_INO).unwrap();
        assert!(!meta.is_dir);
        assert!(!meta.is_symlink);
        assert_eq!(meta.size, 0);
        assert_eq!(meta.nlink, 1);
    }

    #[test]
    fn test_stat_zero() {
        let fs = DevFs;
        let meta = fs.stat(ZERO_INO).unwrap();
        assert!(!meta.is_dir);
        assert_eq!(meta.ino, ZERO_INO);
    }

    #[test]
    fn test_stat_serial() {
        let fs = DevFs;
        let meta = fs.stat(SERIAL_INO).unwrap();
        assert!(!meta.is_dir);
        assert_eq!(meta.ino, SERIAL_INO);
    }

    #[test]
    fn test_stat_console() {
        let fs = DevFs;
        let meta = fs.stat(CONSOLE_INO).unwrap();
        assert!(!meta.is_dir);
        assert_eq!(meta.ino, CONSOLE_INO);
    }

    #[test]
    fn test_stat_random() {
        let fs = DevFs;
        let meta = fs.stat(RANDOM_INO).unwrap();
        assert!(!meta.is_dir);
        assert_eq!(meta.ino, RANDOM_INO);
    }

    #[test]
    fn test_stat_urandom() {
        let fs = DevFs;
        let meta = fs.stat(URANDOM_INO).unwrap();
        assert!(!meta.is_dir);
        assert_eq!(meta.ino, URANDOM_INO);
    }

    #[test]
    fn test_readdir_root_entry_count() {
        let fs = DevFs;
        let entries = fs.readdir(ROOT_INO).unwrap();
        // "." + ".." + 6 devices = 8 entries.
        assert_eq!(entries.len(), 8);
    }

    #[test]
    fn test_readdir_root_entry_order() {
        let fs = DevFs;
        let entries = fs.readdir(ROOT_INO).unwrap();
        // First entry should be ".".
        assert_eq!(entries[0].name, ".");
        assert!(entries[0].is_dir);
        // Second entry should be "..".
        assert_eq!(entries[1].name, "..");
        assert!(entries[1].is_dir);
    }

    #[test]
    fn test_readdir_non_root_directory_not_found() {
        let fs = DevFs;
        assert_eq!(fs.readdir(NULL_INO), Err(FsError::NotFound));
        assert_eq!(fs.readdir(ZERO_INO), Err(FsError::NotFound));
        assert_eq!(fs.readdir(SERIAL_INO), Err(FsError::NotFound));
    }

    #[test]
    fn test_read_with_zero_offset_and_length() {
        let fs = DevFs;
        let mut buf = [];
        let n = fs.read(ZERO_INO, 0, &mut buf).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn test_random_reads_differ_from_urandom() {
        // Both /dev/random and /dev/urandom use the same LCG, so reads at
        // the same time will produce the same data. This just exercises both paths.
        let fs = DevFs;
        let mut buf_random = [0u8; 16];
        let mut buf_urandom = [0u8; 16];
        fs.read(RANDOM_INO, 0, &mut buf_random).unwrap();
        fs.read(URANDOM_INO, 0, &mut buf_urandom).unwrap();
        // These could be equal if the LCG produces sequential values that happen
        // to match after two reads; just verify both are non-deterministically filled.
        assert!(buf_random.iter().any(|&b| b != 0));
        assert!(buf_urandom.iter().any(|&b| b != 0));
    }

    #[test]
    fn test_symlink_not_supported() {
        let fs = DevFs;
        assert_eq!(
            fs.symlink(ROOT_INO, "link", "/target"),
            Err(FsError::NotSupported)
        );
    }

    #[test]
    fn test_readlink_not_supported() {
        let fs = DevFs;
        let mut buf = [0u8; 16];
        assert_eq!(fs.readlink(NULL_INO, &mut buf), Err(FsError::NotSupported));
    }

    #[test]
    fn test_urandom_small_reads() {
        let fs = DevFs;
        let mut buf = [0u8; 1];
        let n = fs.read(URANDOM_INO, 0, &mut buf).unwrap();
        assert_eq!(n, 1);
    }
}
