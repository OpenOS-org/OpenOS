//! Process filesystem (procfs) — virtual filesystem for system and process info.
//!
//! Provides read-only virtual files that expose kernel statistics and
//! per-process information. Content is generated dynamically on read.
//!
//! ## Supported paths
//!
//! - `/proc/meminfo` — frame allocator statistics (total/free memory)
//! - `/proc/uptime` — system uptime in seconds
//! - `/proc/version` — kernel version string
//! - `/proc/[pid]/cmdline` — task command name
//! - `/proc/[pid]/status` — task state and name
//! - `/proc/[pid]/maps` — simplified memory map (VMA list)
//!
//! ## Inode scheme
//!
//! Synthetic inode numbers encode the file type and optional PID:
//! - `1` = root directory (`/proc`)
//! - `2` = `meminfo`
//! - `3` = `uptime`
//! - `4` = `version`
//! - `0x10000 + pid` = per-pid directory
//! - `0x20000 + pid` = `cmdline` for pid
//! - `0x30000 + pid` = `status` for pid
//! - `0x40000 + pid` = `maps` for pid

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write;
use core::sync::atomic::{AtomicU64, Ordering};

use super::vfs::{DirEntry, FileSystem, FsError, InodeMeta, OpenFlags};

// ---------------------------------------------------------------------------
// Inode number constants
// ---------------------------------------------------------------------------

/// Inode number for the procfs root directory.
const ROOT_INO: u64 = 1;

/// Inode number for `/proc/meminfo`.
const MEMINFO_INO: u64 = 2;

/// Inode number for `/proc/uptime`.
const UPTIME_INO: u64 = 3;

/// Inode number for `/proc/version`.
const VERSION_INO: u64 = 4;

/// Inode number for `/proc/net` directory.
const NET_DIR_INO: u64 = 5;

/// Inode number for `/proc/net/ifconfig`.
const NET_IFCONFIG_INO: u64 = 6;

/// Offset for per-pid directory inodes.
const PID_DIR_OFFSET: u64 = 0x1_0000;

/// Offset for per-pid cmdline inodes.
const PID_CMDLINE_OFFSET: u64 = 0x2_0000;

/// Offset for per-pid status inodes.
const PID_STATUS_OFFSET: u64 = 0x3_0000;

/// Offset for per-pid maps inodes.
const PID_MAPS_OFFSET: u64 = 0x4_0000;

// ---------------------------------------------------------------------------
// System tick counter for uptime
// ---------------------------------------------------------------------------

/// Monotonic tick counter incremented by the timer interrupt.
/// Wraps at `u64::MAX` ticks (effectively never in practice).
static TICKS: AtomicU64 = AtomicU64::new(0);

/// Tick frequency (assumes 100 Hz timer, matching typical PIT/APIC rate).
const TICKS_PER_SECOND: u64 = 100;

/// Increment the system tick counter. Called from the timer ISR.
pub fn tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

/// Get the system uptime in seconds.
fn uptime_seconds() -> u64 {
    TICKS.load(Ordering::Relaxed) / TICKS_PER_SECOND
}

// ---------------------------------------------------------------------------
// ProcFs implementation
// ---------------------------------------------------------------------------

/// Process filesystem — implements `FileSystem` to expose kernel state.
pub struct ProcFs;

impl ProcFs {
    /// Parse a path relative to `/proc/` and return the synthetic inode number.
    ///
    /// Returns `None` for unrecognized paths.
    fn resolve_path(path: &str) -> Option<u64> {
        let path = path.trim_start_matches('/');
        if path.is_empty() || path == "." {
            return Some(ROOT_INO);
        }
        match path {
            "meminfo" => Some(MEMINFO_INO),
            "uptime" => Some(UPTIME_INO),
            "version" => Some(VERSION_INO),
            "net" => Some(NET_DIR_INO),
            "net/ifconfig" => Some(NET_IFCONFIG_INO),
            _ => Self::resolve_pid_path(path),
        }
    }

    /// Parse per-pid paths like `123/cmdline`, `123/status`, `123/maps`.
    fn resolve_pid_path(path: &str) -> Option<u64> {
        let mut parts = path.splitn(2, '/');
        let pid_str = parts.next()?;
        let pid: u64 = pid_str.parse().ok()?;
        let sub = parts.next().unwrap_or("");

        match sub {
            "" | "." => Some(PID_DIR_OFFSET + pid),
            "cmdline" => Some(PID_CMDLINE_OFFSET + pid),
            "status" => Some(PID_STATUS_OFFSET + pid),
            "maps" => Some(PID_MAPS_OFFSET + pid),
            _ => None,
        }
    }

    /// Generate the content for `/proc/meminfo`.
    fn read_meminfo() -> String {
        let total_frames = crate::frame_alloc::frame_count();
        let total_kb = total_frames * 4; // 4 KiB per frame

        // Count allocated frames by scanning the bitmap.
        let allocated = Self::count_allocated_frames();
        let free_kb = (total_frames - allocated) * 4;

        let mut out = String::new();
        let _ = writeln!(out, "MemTotal: {} kB", total_kb);
        let _ = writeln!(out, "MemFree: {} kB", free_kb);
        out
    }

    /// Count the number of allocated (set) bits in the frame bitmap.
    ///
    /// Uses a heuristic: the first 256 frames (1 MiB) are reserved during init.
    /// A precise count would require bitmap introspection, which is not yet exposed.
    fn count_allocated_frames() -> usize {
        let total = crate::frame_alloc::frame_count();
        // Heuristic: frames 0-255 are marked reserved during init.
        256.min(total)
    }

    /// Generate the content for `/proc/uptime`.
    fn read_uptime() -> String {
        let secs = uptime_seconds();
        format!("{secs}.00 {secs}.00\n")
    }

    /// Generate the content for `/proc/version`.
    fn read_version() -> String {
        String::from("OpenOS 0.1.0\n")
    }

    /// Generate the content for `/proc/net/ifconfig`.
    ///
    /// Displays network interface configuration including IP address, netmask,
    /// gateway, MAC address, DHCP lease info, and interface statistics.
    fn read_ifconfig() -> String {
        let state = crate::net::dhcp::get_network_state();
        let mac = crate::drivers::net::mac_address();
        let stats = crate::drivers::net::interface_stats();

        let mut out = String::new();

        // Interface header.
        let _ = writeln!(
            out,
            "eth0    Link encap:Ethernet  HWaddr {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
        );

        if state.configured {
            let _ = writeln!(
                out,
                "        inet addr:{}.{}.{}.{}  Mask:{}.{}.{}.{}  Gateway:{}.{}.{}.{}",
                state.ip[0],
                state.ip[1],
                state.ip[2],
                state.ip[3],
                state.subnet_mask[0],
                state.subnet_mask[1],
                state.subnet_mask[2],
                state.subnet_mask[3],
                state.gateway[0],
                state.gateway[1],
                state.gateway[2],
                state.gateway[3]
            );

            let _ = writeln!(
                out,
                "        DNS:{}.{}.{}.{}  DHCP Server:{}.{}.{}.{}",
                state.dns[0],
                state.dns[1],
                state.dns[2],
                state.dns[3],
                state.server_ip[0],
                state.server_ip[1],
                state.server_ip[2],
                state.server_ip[3]
            );

            if state.lease_secs > 0 {
                let ticks = TICKS.load(Ordering::Relaxed);
                let elapsed = ticks.saturating_sub(state.lease_acquired_tick) / TICKS_PER_SECOND;
                let remaining = state.lease_secs.saturating_sub(elapsed as u32);
                let renew_at = state.lease_secs / 2;
                let _ = writeln!(
                    out,
                    "        Lease: {}s  Remaining: {}s  Renew at: {}s (50%)",
                    state.lease_secs, remaining, renew_at
                );
            }

            let _ = writeln!(out, "        Status: UP");
        } else {
            let _ = writeln!(out, "        Status: NOT CONFIGURED (DHCP pending)");
        }

        let _ = writeln!(
            out,
            "        RX packets:{} errors:{} dropped:{}",
            stats.rx_packets, stats.rx_errors, stats.rx_dropped
        );
        let _ = writeln!(
            out,
            "        TX packets:{} errors:{} dropped:{}",
            stats.tx_packets, stats.tx_errors, stats.tx_dropped
        );

        out
    }

    /// Generate the content for `/proc/[pid]/cmdline`.
    fn read_pid_cmdline(pid: u64) -> Result<String, FsError> {
        let task_id = crate::task::task::TaskId::from_u64(pid);
        let name = crate::task::scheduler::with_task(task_id, |task| task.name.clone());
        match name {
            Some(n) => Ok(format!("{n}\n")),
            None => Err(FsError::NotFound),
        }
    }

    /// Generate the content for `/proc/[pid]/status`.
    fn read_pid_status(pid: u64) -> Result<String, FsError> {
        let task_id = crate::task::task::TaskId::from_u64(pid);
        let info = crate::task::scheduler::with_task(task_id, |task| {
            let state = match task.state {
                crate::task::task::TaskState::Ready => "Ready",
                crate::task::task::TaskState::Running => "Running",
                crate::task::task::TaskState::Blocked => "Blocked",
                crate::task::task::TaskState::Terminated => "Terminated",
            };
            (task.name.clone(), state, task.priority)
        });
        match info {
            Some((name, state, prio)) => {
                let mut out = String::new();
                let _ = writeln!(out, "Name: {name}");
                let _ = writeln!(out, "State: {state}");
                let _ = writeln!(out, "Priority: {prio}");
                Ok(out)
            }
            None => Err(FsError::NotFound),
        }
    }

    /// Generate the content for `/proc/[pid]/maps`.
    fn read_pid_maps(pid: u64) -> Result<String, FsError> {
        let task_id = crate::task::task::TaskId::from_u64(pid);
        let maps = crate::task::scheduler::with_task(task_id, |task| {
            let mut out = String::new();
            for vma in task.vma_list.iter() {
                let perms = format!(
                    "{}{}{}",
                    if vma.flags.read { 'r' } else { '-' },
                    if vma.flags.write { 'w' } else { '-' },
                    if vma.flags.execute { 'x' } else { '-' },
                );
                let kind = match vma.kind {
                    crate::memory::vma::VmaType::Code => "code",
                    crate::memory::vma::VmaType::Data => "data",
                    crate::memory::vma::VmaType::Stack => "stack",
                    crate::memory::vma::VmaType::Heap => "heap",
                    crate::memory::vma::VmaType::Mmap => "mmap",
                };
                let _ = writeln!(
                    out,
                    "{:#018x}-{:#018x} {} {}",
                    vma.start,
                    vma.start + vma.size,
                    perms,
                    kind,
                );
            }
            out
        });
        match maps {
            Some(m) => Ok(m),
            None => Err(FsError::NotFound),
        }
    }

    /// Generate directory listing for `/proc` root.
    fn readdir_root() -> Vec<DirEntry> {
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
        entries.push(DirEntry {
            name: String::from("meminfo"),
            ino: MEMINFO_INO,
            is_dir: false,
        });
        entries.push(DirEntry {
            name: String::from("uptime"),
            ino: UPTIME_INO,
            is_dir: false,
        });
        entries.push(DirEntry {
            name: String::from("version"),
            ino: VERSION_INO,
            is_dir: false,
        });
        entries.push(DirEntry {
            name: String::from("net"),
            ino: NET_DIR_INO,
            is_dir: true,
        });

        // Enumerate tasks by scanning all CPU queues.
        // We collect PIDs from the scheduler's task lookup.
        let pids = Self::enumerate_pids();
        for pid in pids {
            entries.push(DirEntry {
                name: pid.to_string(),
                ino: PID_DIR_OFFSET + pid,
                is_dir: true,
            });
        }

        entries
    }

    /// Enumerate all known task PIDs by scanning CPU queues.
    fn enumerate_pids() -> Vec<u64> {
        // Use a static approach: scan a reasonable range of task IDs.
        // Task IDs are monotonically increasing from 0, so we check for
        // existence via the scheduler's with_task.
        let mut pids = Vec::new();
        // Scan up to 256 possible task IDs (matching MAX_TASKS in scheduler).
        for pid in 0..256 {
            let task_id = crate::task::task::TaskId::from_u64(pid);
            let exists = crate::task::scheduler::with_task(task_id, |task| {
                task.state != crate::task::task::TaskState::Terminated
            });
            if exists.unwrap_or(false) {
                pids.push(pid);
            }
        }
        pids
    }
}

impl FileSystem for ProcFs {
    fn open(&self, path: &str, _flags: OpenFlags) -> Result<u64, FsError> {
        Self::resolve_path(path).ok_or(FsError::NotFound)
    }

    fn close(&self, _ino: u64) -> Result<(), FsError> {
        Ok(())
    }

    fn read(&self, ino: u64, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        let content = match ino {
            ROOT_INO => return Err(FsError::NotSupported),
            MEMINFO_INO => Self::read_meminfo(),
            UPTIME_INO => Self::read_uptime(),
            VERSION_INO => Self::read_version(),
            NET_IFCONFIG_INO => Self::read_ifconfig(),
            NET_DIR_INO => return Err(FsError::NotSupported),
            _ if ino >= PID_CMDLINE_OFFSET && ino < PID_STATUS_OFFSET => {
                let pid = ino - PID_CMDLINE_OFFSET;
                Self::read_pid_cmdline(pid)?
            }
            _ if ino >= PID_STATUS_OFFSET && ino < PID_MAPS_OFFSET => {
                let pid = ino - PID_STATUS_OFFSET;
                Self::read_pid_status(pid)?
            }
            _ if ino >= PID_MAPS_OFFSET => {
                let pid = ino - PID_MAPS_OFFSET;
                Self::read_pid_maps(pid)?
            }
            _ if ino >= PID_DIR_OFFSET => return Err(FsError::NotSupported),
            _ => return Err(FsError::NotFound),
        };

        let bytes = content.as_bytes();
        let off = offset as usize;
        if off >= bytes.len() {
            return Ok(0); // EOF
        }
        let available = bytes.len() - off;
        let to_read = buf.len().min(available);
        buf[..to_read].copy_from_slice(&bytes[off..off + to_read]);
        Ok(to_read)
    }

    fn write(&self, _ino: u64, _offset: u64, _data: &[u8]) -> Result<usize, FsError> {
        Err(FsError::PermissionDenied)
    }

    fn stat(&self, ino: u64) -> Result<InodeMeta, FsError> {
        let (is_dir, size) = match ino {
            ROOT_INO => (true, 0),
            MEMINFO_INO => (false, Self::read_meminfo().len() as u64),
            UPTIME_INO => (false, Self::read_uptime().len() as u64),
            VERSION_INO => (false, Self::read_version().len() as u64),
            NET_DIR_INO => (true, 0),
            NET_IFCONFIG_INO => (false, Self::read_ifconfig().len() as u64),
            _ if ino >= PID_DIR_OFFSET && ino < PID_CMDLINE_OFFSET => (true, 0),
            _ if ino >= PID_CMDLINE_OFFSET && ino < PID_STATUS_OFFSET => (false, 0),
            _ if ino >= PID_STATUS_OFFSET && ino < PID_MAPS_OFFSET => (false, 0),
            _ if ino >= PID_MAPS_OFFSET => (false, 0),
            _ => return Err(FsError::NotFound),
        };
        Ok(InodeMeta {
            ino,
            is_dir,
            is_symlink: false,
            size,
            nlink: 1,
        })
    }

    fn readdir(&self, dir_ino: u64) -> Result<Vec<DirEntry>, FsError> {
        match dir_ino {
            ROOT_INO => Ok(Self::readdir_root()),
            NET_DIR_INO => {
                let mut entries = Vec::new();
                entries.push(DirEntry {
                    name: String::from("."),
                    ino: NET_DIR_INO,
                    is_dir: true,
                });
                entries.push(DirEntry {
                    name: String::from(".."),
                    ino: ROOT_INO,
                    is_dir: true,
                });
                entries.push(DirEntry {
                    name: String::from("ifconfig"),
                    ino: NET_IFCONFIG_INO,
                    is_dir: false,
                });
                Ok(entries)
            }
            _ if dir_ino >= PID_DIR_OFFSET && dir_ino < PID_CMDLINE_OFFSET => {
                let pid = dir_ino - PID_DIR_OFFSET;
                // Verify the task exists.
                let task_id = crate::task::task::TaskId::from_u64(pid);
                let exists = crate::task::scheduler::with_task(task_id, |_| ());
                if exists.is_none() {
                    return Err(FsError::NotFound);
                }
                let mut entries = Vec::new();
                entries.push(DirEntry {
                    name: String::from("."),
                    ino: dir_ino,
                    is_dir: true,
                });
                entries.push(DirEntry {
                    name: String::from(".."),
                    ino: ROOT_INO,
                    is_dir: true,
                });
                entries.push(DirEntry {
                    name: String::from("cmdline"),
                    ino: PID_CMDLINE_OFFSET + pid,
                    is_dir: false,
                });
                entries.push(DirEntry {
                    name: String::from("status"),
                    ino: PID_STATUS_OFFSET + pid,
                    is_dir: false,
                });
                entries.push(DirEntry {
                    name: String::from("maps"),
                    ino: PID_MAPS_OFFSET + pid,
                    is_dir: false,
                });
                Ok(entries)
            }
            _ => Err(FsError::NotFound),
        }
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
        assert_eq!(ProcFs::resolve_path(""), Some(ROOT_INO));
        assert_eq!(ProcFs::resolve_path("."), Some(ROOT_INO));
    }

    #[test]
    fn test_resolve_static_files() {
        assert_eq!(ProcFs::resolve_path("meminfo"), Some(MEMINFO_INO));
        assert_eq!(ProcFs::resolve_path("uptime"), Some(UPTIME_INO));
        assert_eq!(ProcFs::resolve_path("version"), Some(VERSION_INO));
    }

    #[test]
    fn test_resolve_pid_paths() {
        assert_eq!(ProcFs::resolve_path("42"), Some(PID_DIR_OFFSET + 42));
        assert_eq!(
            ProcFs::resolve_path("42/cmdline"),
            Some(PID_CMDLINE_OFFSET + 42)
        );
        assert_eq!(
            ProcFs::resolve_path("42/status"),
            Some(PID_STATUS_OFFSET + 42)
        );
        assert_eq!(ProcFs::resolve_path("42/maps"), Some(PID_MAPS_OFFSET + 42));
    }

    #[test]
    fn test_resolve_unknown_returns_none() {
        assert_eq!(ProcFs::resolve_path("nonexistent"), None);
        assert_eq!(ProcFs::resolve_path("42/bogus"), None);
    }

    #[test]
    fn test_version_content() {
        assert_eq!(ProcFs::read_version(), "OpenOS 0.1.0\n");
    }

    #[test]
    fn test_uptime_format() {
        let uptime = ProcFs::read_uptime();
        assert!(uptime.ends_with('\n'));
        // Should contain at least one digit.
        assert!(uptime.chars().any(|c| c.is_ascii_digit()));
    }

    #[test]
    fn test_meminfo_format() {
        let info = ProcFs::read_meminfo();
        assert!(info.contains("MemTotal:"));
        assert!(info.contains("MemFree:"));
        assert!(info.contains("kB"));
    }

    #[test]
    fn test_open_and_close() {
        let fs = ProcFs;
        let ino = fs.open("version", OpenFlags::READ).unwrap();
        assert_eq!(ino, VERSION_INO);
        fs.close(ino).unwrap();
    }

    #[test]
    fn test_open_not_found() {
        let fs = ProcFs;
        assert_eq!(
            fs.open("nonexistent", OpenFlags::READ),
            Err(FsError::NotFound)
        );
    }

    #[test]
    fn test_read_version() {
        let fs = ProcFs;
        let mut buf = [0u8; 64];
        let n = fs.read(VERSION_INO, 0, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"OpenOS 0.1.0\n");
    }

    #[test]
    fn test_read_at_offset() {
        let fs = ProcFs;
        let mut buf = [0u8; 64];
        // "OpenOS 0.1.0\n" — offset 6 = " 0.1.0\n"
        let n = fs.read(VERSION_INO, 6, &mut buf).unwrap();
        assert_eq!(&buf[..n], b" 0.1.0\n");
    }

    #[test]
    fn test_read_eof() {
        let fs = ProcFs;
        let mut buf = [0u8; 64];
        let n = fs.read(VERSION_INO, 1000, &mut buf).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn test_write_denied() {
        let fs = ProcFs;
        assert_eq!(
            fs.write(VERSION_INO, 0, b"hack"),
            Err(FsError::PermissionDenied)
        );
    }

    #[test]
    fn test_create_denied() {
        let fs = ProcFs;
        assert_eq!(fs.create(ROOT_INO, "bad"), Err(FsError::PermissionDenied));
    }

    #[test]
    fn test_unlink_denied() {
        let fs = ProcFs;
        assert_eq!(fs.unlink(ROOT_INO, "bad"), Err(FsError::PermissionDenied));
    }

    #[test]
    fn test_stat_root() {
        let fs = ProcFs;
        let meta = fs.stat(ROOT_INO).unwrap();
        assert!(meta.is_dir);
        assert_eq!(meta.ino, ROOT_INO);
    }

    #[test]
    fn test_stat_version() {
        let fs = ProcFs;
        let meta = fs.stat(VERSION_INO).unwrap();
        assert!(!meta.is_dir);
    }

    #[test]
    fn test_stat_unknown() {
        let fs = ProcFs;
        assert_eq!(fs.stat(9999), Err(FsError::NotFound));
    }

    #[test]
    fn test_readdir_root() {
        let fs = ProcFs;
        let entries = fs.readdir(ROOT_INO).unwrap();
        assert!(entries.iter().any(|e| e.name == "."));
        assert!(entries.iter().any(|e| e.name == ".."));
        assert!(entries.iter().any(|e| e.name == "meminfo"));
        assert!(entries.iter().any(|e| e.name == "uptime"));
        assert!(entries.iter().any(|e| e.name == "version"));
    }

    #[test]
    fn test_readdir_non_root_not_found() {
        let fs = ProcFs;
        assert_eq!(fs.readdir(9999), Err(FsError::NotFound));
    }

    #[test]
    fn test_tick_counter() {
        let before = TICKS.load(Ordering::Relaxed);
        tick();
        let after = TICKS.load(Ordering::Relaxed);
        assert_eq!(after, before + 1);
    }

    #[test]
    fn test_inode_encoding_roundtrip() {
        // Verify that PID paths produce distinct inode numbers.
        let pid = 7;
        let dir = PID_DIR_OFFSET + pid;
        let cmdline = PID_CMDLINE_OFFSET + pid;
        let status = PID_STATUS_OFFSET + pid;
        let maps = PID_MAPS_OFFSET + pid;
        assert_ne!(dir, cmdline);
        assert_ne!(cmdline, status);
        assert_ne!(status, maps);
        // Verify decoding.
        assert_eq!(cmdline - PID_CMDLINE_OFFSET, pid);
        assert_eq!(status - PID_STATUS_OFFSET, pid);
        assert_eq!(maps - PID_MAPS_OFFSET, pid);
    }
}
