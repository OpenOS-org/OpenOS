//! Process filesystem (procfs) — virtual filesystem for system and process info.
//!
//! Provides read-only virtual files that expose kernel statistics and
//! per-process information. Content is generated dynamically on read.
//!
//! ## Supported paths
//!
//! - `/proc/meminfo` — frame allocator statistics (total/free memory)
//! - `/proc/cpuinfo` — CPU model, cores, and frequency
//! - `/proc/uptime` — system uptime in seconds
//! - `/proc/version` — kernel version string
//! - `/proc/[pid]/cmdline` — task command name
//! - `/proc/[pid]/status` — task state and name
//! - `/proc/[pid]/maps` — simplified memory map (VMA list)
//! - `/proc/net/tcp` — list of TCP connections
//! - `/proc/net/udp` — list of UDP sockets
//! - `/proc/net/ifconfig` — network interface configuration
//! - `/proc/[pid]/fd` — directory listing open file descriptors
//! - `/proc/[pid]/environ` — environment variables (null-separated)
//!
//! ## Inode scheme
//!
//! Synthetic inode numbers encode the file type and optional PID:
//! - `1` = root directory (`/proc`)
//! - `2` = `meminfo`
//! - `3` = `uptime`
//! - `4` = `version`
//! - `5` = `net` directory
//! - `6` = `net/ifconfig`
//! - `7` = `net/tcp`
//! - `8` = `net/udp`
//! - `9` = `cpuinfo`
//! - `10` = `net/arp`
//! - `11` = `net/dev`
//! - `0x10000 + pid` = per-pid directory
//! - `0x20000 + pid` = `cmdline` for pid
//! - `0x30000 + pid` = `status` for pid
//! - `0x40000 + pid` = `maps` for pid
//! - `0x50000 + pid` = `fd` directory for pid
//! - `0x60000 + pid` = `environ` file for pid

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::{format, vec};
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

/// Inode number for `/proc/net/tcp`.
const NET_TCP_INO: u64 = 7;

/// Inode number for `/proc/net/udp`.
const NET_UDP_INO: u64 = 8;

/// Inode number for `/proc/cpuinfo`.
const CPUINFO_INO: u64 = 9;

/// Inode number for `/proc/net/arp`.
const NET_ARP_INO: u64 = 10;

/// Inode number for `/proc/net/dev`.
const NET_DEV_INO: u64 = 11;

/// Offset for per-pid directory inodes.
const PID_DIR_OFFSET: u64 = 0x1_0000;

/// Offset for per-pid cmdline inodes.
const PID_CMDLINE_OFFSET: u64 = 0x2_0000;

/// Offset for per-pid status inodes.
const PID_STATUS_OFFSET: u64 = 0x3_0000;

/// Offset for per-pid maps inodes.
const PID_MAPS_OFFSET: u64 = 0x4_0000;

/// Offset for per-pid fd directory inodes.
const PID_FD_OFFSET: u64 = 0x5_0000;

/// Offset for per-pid environ file inodes.
const PID_ENVIRON_OFFSET: u64 = 0x6_0000;

// Maximum number of FD entries to list per process in /proc/[pid]/fd.
const MAX_FD_ENTRIES: usize = 256;

// ---------------------------------------------------------------------------
// System tick counter for uptime
// ---------------------------------------------------------------------------

/// Format an IPv4 address as a dotted-quad string.
fn format_ip(ip: u32) -> alloc::string::String {
    alloc::format!(
        "{}.{}.{}.{}",
        (ip >> 24) & 0xFF,
        (ip >> 16) & 0xFF,
        (ip >> 8) & 0xFF,
        ip & 0xFF
    )
}

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
            "cpuinfo" => Some(CPUINFO_INO),
            "net" => Some(NET_DIR_INO),
            "net/ifconfig" => Some(NET_IFCONFIG_INO),
            "net/tcp" => Some(NET_TCP_INO),
            "net/udp" => Some(NET_UDP_INO),
            "net/arp" => Some(NET_ARP_INO),
            "net/dev" => Some(NET_DEV_INO),
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
            "fd" => Some(PID_FD_OFFSET + pid),
            "environ" => Some(PID_ENVIRON_OFFSET + pid),
            _ => Self::resolve_fd_entry_path(pid, sub),
        }
    }

    /// Generate the content for `/proc/meminfo`.
    fn read_meminfo() -> String {
        let total_frames = crate::frame_alloc::frame_count();
        let region_start = crate::frame_alloc::frame_region_start();
        let region_end = crate::frame_alloc::frame_region_end();
        let total_kb = total_frames * 4; // 4 KiB per frame

        // Estimate reserved frames (first 256 frames = 1 MiB are reserved).
        let reserved = 256.min(total_frames);
        let free_kb = (total_frames - reserved) * 4;
        let used_kb = total_kb - free_kb;

        let mut out = String::new();
        let _ = writeln!(out, "MemTotal:       {total_kb} kB");
        let _ = writeln!(out, "MemFree:        {free_kb} kB");
        let _ = writeln!(out, "MemUsed:        {used_kb} kB");
        let _ = writeln!(out, "FrameRegion:    {region_start:#x}-{region_end:#x}");
        let _ = writeln!(out, "FrameCount:     {total_frames}");
        let _ = writeln!(out, "FrameSize:      4096 bytes");
        out
    }

    /// Generate the content for `/proc/cpuinfo`.
    ///
    /// Reports CPU model, number of cores, and frequency. In a real system
    /// this would read CPUID and ACPI MADT; here we report what we know.
    fn read_cpuinfo() -> String {
        let num_cpus = crate::arch::x86_64::percpu::cpu_count();
        let mut out = String::new();

        for i in 0..num_cpus {
            let _ = writeln!(out, "processor\t: {i}");
            let _ = writeln!(out, "vendor_id\t: OpenOS");
            let _ = writeln!(out, "model name\t: x86_64 Virtual CPU");
            let _ = writeln!(out, "cpu cores\t: {num_cpus}");
            // LAPIC timer frequency is not exposed here; report a placeholder.
            let _ = writeln!(out, "cpu MHz\t\t: unknown");
            let _ = writeln!(out);
        }
        out
    }

    /// Generate the content for `/proc/uptime`.
    fn read_uptime() -> String {
        let secs = uptime_seconds();
        format!("{secs}.00 {secs}.00\n")
    }

    /// Generate the content for `/proc/version`.
    fn read_version() -> String {
        String::from("OpenOS 0.3.0\n")
    }

    /// Generate the content for `/proc/net/ifconfig`.
    ///
    /// Displays network interface configuration including IP address, netmask,
    /// gateway, MAC address, and DHCP lease info.
    fn read_ifconfig() -> String {
        let state = crate::net::dhcp::get_network_state();
        let mac = crate::drivers::net::mac_address();

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

        out
    }

    /// Generate the content for `/proc/net/tcp`.
    ///
    /// Lists all active TCP connections in a format similar to Linux's
    /// `/proc/net/tcp`: `sl  local_address  remote_address  state`.
    fn read_tcp() -> String {
        let connections = crate::net::tcp::list_connections();
        let mut out = String::new();

        let _ = writeln!(out, "sl  local_address  remote_address  state");

        for (i, conn) in connections.iter().enumerate() {
            let state_str = match conn.state {
                crate::net::tcp::TcpState::Closed => "CLOSED",
                crate::net::tcp::TcpState::SynSent => "SYN_SENT",
                crate::net::tcp::TcpState::SynReceived => "SYN_RECV",
                crate::net::tcp::TcpState::Established => "ESTABLISHED",
                crate::net::tcp::TcpState::FinWait1 => "FIN_WAIT1",
                crate::net::tcp::TcpState::FinWait2 => "FIN_WAIT2",
                crate::net::tcp::TcpState::CloseWait => "CLOSE_WAIT",
                crate::net::tcp::TcpState::LastAck => "LAST_ACK",
                crate::net::tcp::TcpState::TimeWait => "TIME_WAIT",
                crate::net::tcp::TcpState::Closing => "CLOSING",
            };

            // Format addresses as IP:port.
            let ra = conn.remote_addr;
            let _ = writeln!(
                out,
                "{:<3} 0.0.0.0:{:<5} {}.{}.{}.{}:{:<5} {}",
                i,
                conn.local_port,
                (ra >> 24) & 0xFF,
                (ra >> 16) & 0xFF,
                (ra >> 8) & 0xFF,
                ra & 0xFF,
                conn.remote_port,
                state_str,
            );
        }

        if connections.is_empty() {
            let _ = writeln!(out);
        }

        out
    }

    /// Generate the content for `/proc/net/udp`.
    ///
    /// UDP has no persistent connection table, so this always shows an empty
    /// list (similar to a freshly-booted Linux system with no UDP sockets).
    fn read_udp() -> String {
        let mut out = String::new();
        let _ = writeln!(out, "sl  local_address  remote_address  state");
        let _ = writeln!(out);
        out
    }

    /// Generate the content for `/proc/net/arp`.
    ///
    /// Lists the ARP table entries in a format similar to Linux:
    /// `IP address  HW type  Flags  HW address  Device`.
    fn read_arp() -> String {
        let entries = crate::net::get_arp_table();
        let mut out = String::new();
        let _ = writeln!(
            out,
            "IP address       HW type  Flags  HW address        Device"
        );
        for (ip, mac) in &entries {
            let _ = writeln!(
                out,
                "{:<16} 0x1      0x2    {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}  eth0",
                format_ip(*ip),
                mac[0],
                mac[1],
                mac[2],
                mac[3],
                mac[4],
                mac[5],
            );
        }
        if entries.is_empty() {
            let _ = writeln!(out);
        }
        out
    }

    /// Generate the content for `/proc/net/dev`.
    ///
    /// Shows network device statistics.
    fn read_dev() -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "Inter-|   Receive                                                |  Transmit"
        );
        let _ = writeln!(out, " face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo frame compressed");
        let _ = writeln!(
            out,
            "    0:        0       0    0    0    0     0          0         0        0       0    0    0    0     0          0"
        );
        out
    }

    /// Generate the content for `/proc/[pid]/cmdline`.
    fn read_pid_cmdline(pid: u64) -> Result<String, FsError> {
        let task_id = crate::task::task::TaskId::from_u64(pid);
        crate::task::scheduler::with_task(task_id, |task| task.name.clone())
            .ok_or(FsError::NotFound)
            .map(|n| format!("{n}\n"))
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
            for vma in &task.vma_list {
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
        maps.ok_or(FsError::NotFound)
    }

    /// Parse paths like `123/fd/3` — individual FD entries within a pid's fd directory.
    ///
    /// Returns a synthetic inode encoding `(pid, fd_number)`.
    /// FD entry inodes are encoded as: `PID_FD_OFFSET + pid + (fd << 32)`.
    /// Since `pid` uses only the low 32 bits and `fd` uses bits 32-63, there
    /// is no overlap for reasonable PID/FD values.
    fn resolve_fd_entry_path(pid: u64, sub: &str) -> Option<u64> {
        // sub should be "fd/<number>"
        let mut parts = sub.splitn(2, '/');
        let prefix = parts.next()?;
        if prefix != "fd" {
            return None;
        }
        let fd_str = parts.next()?;
        let fd: u64 = fd_str.parse().ok()?;
        Some(PID_FD_OFFSET + pid + (fd << 32))
    }

    /// Generate the content for `/proc/[pid]/environ`.
    ///
    /// Returns null-separated `key=value` pairs, matching Linux convention.
    fn read_pid_environ(pid: u64) -> Result<String, FsError> {
        let task_id = crate::task::task::TaskId::from_u64(pid);
        let env = crate::task::scheduler::with_task(task_id, |task| {
            let mut out = String::new();
            for (key, value) in &task.env {
                if !out.is_empty() {
                    out.push('\0');
                }
                let _ = write!(out, "{key}={value}");
            }
            out
        });
        env.ok_or(FsError::NotFound)
    }

    /// Generate the content for `/proc/[pid]/fd/<fd_number>` — shows the path for
    /// a given file descriptor as a text entry.
    fn read_pid_fd_entry(pid: u64, fd_num: u64) -> Result<String, FsError> {
        let task_id = crate::task::task::TaskId::from_u64(pid);
        let path = crate::task::scheduler::with_task(task_id, |task| {
            task.fd_table.get(&fd_num).map(|entry| entry.path.clone())
        });
        match path {
            Some(Some(p)) => Ok(format!("{p}\n")),
            _ => Err(FsError::NotFound),
        }
    }

    /// Generate directory listing for `/proc/[pid]/fd`.
    ///
    /// Lists each open file descriptor number as a directory entry.
    fn readdir_pid_fd(pid: u64) -> Result<Vec<DirEntry>, FsError> {
        let task_id = crate::task::task::TaskId::from_u64(pid);
        let fds = crate::task::scheduler::with_task(task_id, |task| {
            let mut entries = Vec::new();
            entries.push(DirEntry {
                name: String::from("."),
                ino: PID_FD_OFFSET + pid,
                is_dir: true,
            });
            entries.push(DirEntry {
                name: String::from(".."),
                ino: PID_DIR_OFFSET + pid,
                is_dir: true,
            });
            // List each fd number as an entry.
            for (count, &fd_num) in task.fd_table.keys().enumerate() {
                if count >= MAX_FD_ENTRIES {
                    break;
                }
                entries.push(DirEntry {
                    name: fd_num.to_string(),
                    ino: PID_FD_OFFSET + pid + (fd_num << 32),
                    is_dir: false,
                });
            }
            entries
        });
        fds.ok_or(FsError::NotFound)
    }

    /// Generate directory listing for `/proc` root.
    fn readdir_root() -> Vec<DirEntry> {
        let mut entries = vec![
            DirEntry {
                name: String::from("."),
                ino: ROOT_INO,
                is_dir: true,
            },
            DirEntry {
                name: String::from(".."),
                ino: ROOT_INO,
                is_dir: true,
            },
            DirEntry {
                name: String::from("meminfo"),
                ino: MEMINFO_INO,
                is_dir: false,
            },
            DirEntry {
                name: String::from("cpuinfo"),
                ino: CPUINFO_INO,
                is_dir: false,
            },
            DirEntry {
                name: String::from("uptime"),
                ino: UPTIME_INO,
                is_dir: false,
            },
            DirEntry {
                name: String::from("version"),
                ino: VERSION_INO,
                is_dir: false,
            },
            DirEntry {
                name: String::from("net"),
                ino: NET_DIR_INO,
                is_dir: true,
            },
        ];

        // Enumerate tasks by scanning all CPU queues.
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

    /// Generate directory listing for `/proc/net`.
    fn readdir_net() -> Vec<DirEntry> {
        vec![
            DirEntry {
                name: String::from("."),
                ino: NET_DIR_INO,
                is_dir: true,
            },
            DirEntry {
                name: String::from(".."),
                ino: ROOT_INO,
                is_dir: true,
            },
            DirEntry {
                name: String::from("ifconfig"),
                ino: NET_IFCONFIG_INO,
                is_dir: false,
            },
            DirEntry {
                name: String::from("tcp"),
                ino: NET_TCP_INO,
                is_dir: false,
            },
            DirEntry {
                name: String::from("udp"),
                ino: NET_UDP_INO,
                is_dir: false,
            },
            DirEntry {
                name: String::from("arp"),
                ino: NET_ARP_INO,
                is_dir: false,
            },
            DirEntry {
                name: String::from("dev"),
                ino: NET_DEV_INO,
                is_dir: false,
            },
        ]
    }

    /// Enumerate all known task PIDs by scanning CPU queues.
    fn enumerate_pids() -> Vec<u64> {
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
            ROOT_INO | NET_DIR_INO => return Err(FsError::NotSupported),
            MEMINFO_INO => Self::read_meminfo(),
            UPTIME_INO => Self::read_uptime(),
            VERSION_INO => Self::read_version(),
            CPUINFO_INO => Self::read_cpuinfo(),
            NET_IFCONFIG_INO => Self::read_ifconfig(),
            NET_TCP_INO => Self::read_tcp(),
            NET_UDP_INO => Self::read_udp(),
            NET_ARP_INO => Self::read_arp(),
            NET_DEV_INO => Self::read_dev(),
            _ if (PID_CMDLINE_OFFSET..PID_STATUS_OFFSET).contains(&ino) => {
                let pid = ino - PID_CMDLINE_OFFSET;
                Self::read_pid_cmdline(pid)?
            }
            _ if (PID_STATUS_OFFSET..PID_MAPS_OFFSET).contains(&ino) => {
                let pid = ino - PID_STATUS_OFFSET;
                Self::read_pid_status(pid)?
            }
            _ if (PID_MAPS_OFFSET..PID_FD_OFFSET).contains(&ino) => {
                let pid = ino - PID_MAPS_OFFSET;
                Self::read_pid_maps(pid)?
            }
            _ if (PID_FD_OFFSET..PID_ENVIRON_OFFSET).contains(&ino) => {
                // FD entry inodes encode fd in the upper bits: ino = base + pid + (fd << 32).
                let base = ino - PID_FD_OFFSET;
                let pid = base & 0xFFFF_FFFF;
                let fd_num = base >> 32;
                if fd_num == 0 {
                    // Reading the fd directory itself is not supported (it's a directory).
                    return Err(FsError::NotSupported);
                }
                Self::read_pid_fd_entry(pid, fd_num)?
            }
            _ if ino >= PID_ENVIRON_OFFSET => {
                let pid = ino - PID_ENVIRON_OFFSET;
                Self::read_pid_environ(pid)?
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
            ROOT_INO | NET_DIR_INO => (true, 0),
            MEMINFO_INO => (false, Self::read_meminfo().len() as u64),
            UPTIME_INO => (false, Self::read_uptime().len() as u64),
            VERSION_INO => (false, Self::read_version().len() as u64),
            CPUINFO_INO => (false, Self::read_cpuinfo().len() as u64),
            NET_IFCONFIG_INO => (false, Self::read_ifconfig().len() as u64),
            NET_TCP_INO => (false, Self::read_tcp().len() as u64),
            NET_UDP_INO => (false, Self::read_udp().len() as u64),
            NET_ARP_INO => (false, Self::read_arp().len() as u64),
            NET_DEV_INO => (false, Self::read_dev().len() as u64),
            _ if (PID_DIR_OFFSET..PID_CMDLINE_OFFSET).contains(&ino) => (true, 0),
            _ if (PID_CMDLINE_OFFSET..PID_STATUS_OFFSET).contains(&ino) => (false, 0),
            _ if (PID_STATUS_OFFSET..PID_MAPS_OFFSET).contains(&ino) => (false, 0),
            _ if (PID_MAPS_OFFSET..PID_FD_OFFSET).contains(&ino) => (false, 0),
            _ if (PID_FD_OFFSET..PID_ENVIRON_OFFSET).contains(&ino) => {
                let base = ino - PID_FD_OFFSET;
                let fd_num = base >> 32;
                if fd_num == 0 {
                    // PID_FD_OFFSET + pid is the fd directory itself.
                    (true, 0)
                } else {
                    // Individual fd entries are regular files.
                    (false, 0)
                }
            }
            _ if ino >= PID_ENVIRON_OFFSET => (false, 0),
            _ => return Err(FsError::NotFound),
        };
        Ok(InodeMeta {
            ino,
            is_dir,
            is_symlink: false,
            is_fifo: false,
            size,
            nlink: 1,
        })
    }

    fn readdir(&self, dir_ino: u64) -> Result<Vec<DirEntry>, FsError> {
        match dir_ino {
            ROOT_INO => Ok(Self::readdir_root()),
            NET_DIR_INO => Ok(Self::readdir_net()),
            _ if (PID_DIR_OFFSET..PID_CMDLINE_OFFSET).contains(&dir_ino) => {
                let pid = dir_ino - PID_DIR_OFFSET;
                // Verify the task exists.
                let task_id = crate::task::task::TaskId::from_u64(pid);
                let exists = crate::task::scheduler::with_task(task_id, |_| ());
                if exists.is_none() {
                    return Err(FsError::NotFound);
                }
                let entries = vec![
                    DirEntry {
                        name: String::from("."),
                        ino: dir_ino,
                        is_dir: true,
                    },
                    DirEntry {
                        name: String::from(".."),
                        ino: ROOT_INO,
                        is_dir: true,
                    },
                    DirEntry {
                        name: String::from("cmdline"),
                        ino: PID_CMDLINE_OFFSET + pid,
                        is_dir: false,
                    },
                    DirEntry {
                        name: String::from("status"),
                        ino: PID_STATUS_OFFSET + pid,
                        is_dir: false,
                    },
                    DirEntry {
                        name: String::from("maps"),
                        ino: PID_MAPS_OFFSET + pid,
                        is_dir: false,
                    },
                    DirEntry {
                        name: String::from("fd"),
                        ino: PID_FD_OFFSET + pid,
                        is_dir: true,
                    },
                    DirEntry {
                        name: String::from("environ"),
                        ino: PID_ENVIRON_OFFSET + pid,
                        is_dir: false,
                    },
                ];
                Ok(entries)
            }
            _ if (PID_FD_OFFSET..PID_ENVIRON_OFFSET).contains(&dir_ino) => {
                let base = dir_ino - PID_FD_OFFSET;
                let fd_num = base >> 32;
                if fd_num != 0 {
                    // Individual fd entries are not directories.
                    return Err(FsError::NotADirectory);
                }
                let pid = base & 0xFFFF_FFFF;
                Self::readdir_pid_fd(pid)
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

    // --- Path resolution tests ---

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
        assert_eq!(ProcFs::resolve_path("cpuinfo"), Some(CPUINFO_INO));
    }

    #[test]
    fn test_resolve_net_paths() {
        assert_eq!(ProcFs::resolve_path("net"), Some(NET_DIR_INO));
        assert_eq!(ProcFs::resolve_path("net/ifconfig"), Some(NET_IFCONFIG_INO));
        assert_eq!(ProcFs::resolve_path("net/tcp"), Some(NET_TCP_INO));
        assert_eq!(ProcFs::resolve_path("net/udp"), Some(NET_UDP_INO));
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

    // --- Content generation tests ---

    #[test]
    fn test_version_content() {
        assert_eq!(ProcFs::read_version(), "OpenOS 0.3.0\n");
    }

    #[test]
    fn test_uptime_format() {
        let uptime = ProcFs::read_uptime();
        assert!(uptime.ends_with('\n'));
        assert!(uptime.chars().any(|c| c.is_ascii_digit()));
    }

    #[test]
    fn test_meminfo_format() {
        let info = ProcFs::read_meminfo();
        assert!(info.contains("MemTotal:"));
        assert!(info.contains("MemFree:"));
        assert!(info.contains("MemUsed:"));
        assert!(info.contains("kB"));
        assert!(info.contains("FrameCount:"));
    }

    #[test]
    fn test_cpuinfo_format() {
        let info = ProcFs::read_cpuinfo();
        assert!(info.contains("processor"));
        assert!(info.contains("vendor_id"));
        assert!(info.contains("model name"));
        assert!(info.contains("cpu cores"));
    }

    #[test]
    fn test_tcp_format() {
        let tcp = ProcFs::read_tcp();
        assert!(tcp.contains("local_address"));
        assert!(tcp.contains("remote_address"));
        assert!(tcp.contains("state"));
    }

    #[test]
    fn test_udp_format() {
        let udp = ProcFs::read_udp();
        assert!(udp.contains("local_address"));
        assert!(udp.contains("remote_address"));
    }

    // --- FileSystem trait tests ---

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
        assert_eq!(&buf[..n], b"OpenOS 0.3.0\n");
    }

    #[test]
    fn test_read_at_offset() {
        let fs = ProcFs;
        let mut buf = [0u8; 64];
        // "OpenOS 0.3.0\n" -- offset 6 = " 0.3.0\n"
        let n = fs.read(VERSION_INO, 6, &mut buf).unwrap();
        assert_eq!(&buf[..n], b" 0.3.0\n");
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
    fn test_symlink_not_supported() {
        let fs = ProcFs;
        assert_eq!(
            fs.symlink(ROOT_INO, "link", "/target"),
            Err(FsError::NotSupported)
        );
    }

    #[test]
    fn test_readlink_not_supported() {
        let fs = ProcFs;
        let mut buf = [0u8; 16];
        assert_eq!(
            fs.readlink(MEMINFO_INO, &mut buf),
            Err(FsError::NotSupported)
        );
    }

    // --- Stat tests ---

    #[test]
    fn test_stat_root() {
        let fs = ProcFs;
        let meta = fs.stat(ROOT_INO).unwrap();
        assert!(meta.is_dir);
        assert!(!meta.is_symlink);
        assert_eq!(meta.ino, ROOT_INO);
    }

    #[test]
    fn test_stat_version() {
        let fs = ProcFs;
        let meta = fs.stat(VERSION_INO).unwrap();
        assert!(!meta.is_dir);
        assert!(!meta.is_symlink);
        assert!(meta.size > 0);
    }

    #[test]
    fn test_stat_meminfo() {
        let fs = ProcFs;
        let meta = fs.stat(MEMINFO_INO).unwrap();
        assert!(!meta.is_dir);
        assert!(meta.size > 0);
    }

    #[test]
    fn test_stat_cpuinfo() {
        let fs = ProcFs;
        let meta = fs.stat(CPUINFO_INO).unwrap();
        assert!(!meta.is_dir);
        assert!(meta.size > 0);
    }

    #[test]
    fn test_stat_net_dir() {
        let fs = ProcFs;
        let meta = fs.stat(NET_DIR_INO).unwrap();
        assert!(meta.is_dir);
    }

    #[test]
    fn test_stat_net_tcp() {
        let fs = ProcFs;
        let meta = fs.stat(NET_TCP_INO).unwrap();
        assert!(!meta.is_dir);
    }

    #[test]
    fn test_stat_unknown() {
        let fs = ProcFs;
        assert_eq!(fs.stat(9999), Err(FsError::NotFound));
    }

    // --- Readdir tests ---

    #[test]
    fn test_readdir_root() {
        let fs = ProcFs;
        let entries = fs.readdir(ROOT_INO).unwrap();
        assert!(entries.iter().any(|e| e.name == "."));
        assert!(entries.iter().any(|e| e.name == ".."));
        assert!(entries.iter().any(|e| e.name == "meminfo"));
        assert!(entries.iter().any(|e| e.name == "cpuinfo"));
        assert!(entries.iter().any(|e| e.name == "uptime"));
        assert!(entries.iter().any(|e| e.name == "version"));
        assert!(entries.iter().any(|e| e.name == "net"));
    }

    #[test]
    fn test_readdir_net() {
        let fs = ProcFs;
        let entries = fs.readdir(NET_DIR_INO).unwrap();
        assert!(entries.iter().any(|e| e.name == "."));
        assert!(entries.iter().any(|e| e.name == ".."));
        assert!(entries.iter().any(|e| e.name == "ifconfig"));
        assert!(entries.iter().any(|e| e.name == "tcp"));
        assert!(entries.iter().any(|e| e.name == "udp"));
    }

    #[test]
    fn test_readdir_non_root_not_found() {
        let fs = ProcFs;
        assert_eq!(fs.readdir(9999), Err(FsError::NotFound));
    }

    // --- Tick counter test ---

    #[test]
    fn test_tick_counter() {
        let before = TICKS.load(Ordering::Relaxed);
        tick();
        let after = TICKS.load(Ordering::Relaxed);
        assert_eq!(after, before + 1);
    }

    // --- Inode encoding tests ---

    #[test]
    fn test_inode_encoding_roundtrip() {
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

    #[test]
    fn test_static_inodes_distinct() {
        let inodes = [
            ROOT_INO,
            MEMINFO_INO,
            UPTIME_INO,
            VERSION_INO,
            NET_DIR_INO,
            NET_IFCONFIG_INO,
            NET_TCP_INO,
            NET_UDP_INO,
            CPUINFO_INO,
        ];
        for i in 0..inodes.len() {
            for j in (i + 1)..inodes.len() {
                assert_ne!(inodes[i], inodes[j], "inode collision: {} vs {}", i, j);
            }
        }
    }

    // --- fd and environ tests ---

    #[test]
    fn test_inode_encoding_roundtrip_extended() {
        let pid = 7;
        let fd = PID_FD_OFFSET + pid;
        let environ = PID_ENVIRON_OFFSET + pid;
        let maps = PID_MAPS_OFFSET + pid;
        assert_ne!(maps, fd);
        assert_ne!(fd, environ);
        assert_eq!(fd - PID_FD_OFFSET, pid);
        assert_eq!(environ - PID_ENVIRON_OFFSET, pid);
    }

    #[test]
    fn test_resolve_fd_path() {
        assert_eq!(ProcFs::resolve_path("42/fd"), Some(PID_FD_OFFSET + 42));
    }

    #[test]
    fn test_resolve_environ_path() {
        assert_eq!(
            ProcFs::resolve_path("42/environ"),
            Some(PID_ENVIRON_OFFSET + 42)
        );
    }

    #[test]
    fn test_resolve_fd_entry_path() {
        // "42/fd/3" should encode pid=42, fd=3.
        let ino = ProcFs::resolve_path("42/fd/3").unwrap();
        assert_eq!(ino, PID_FD_OFFSET + 42 + (3u64 << 32));
        // Verify round-trip decode.
        let base = ino - PID_FD_OFFSET;
        assert_eq!(base & 0xFFFF_FFFF, 42); // pid
        assert_eq!(base >> 32, 3); // fd number
    }

    #[test]
    fn test_resolve_fd_entry_zero() {
        // "42/fd/0" — fd 0 (stdin).
        let ino = ProcFs::resolve_path("42/fd/0").unwrap();
        let base = ino - PID_FD_OFFSET;
        assert_eq!(base & 0xFFFF_FFFF, 42);
        assert_eq!(base >> 32, 0);
    }

    #[test]
    fn test_resolve_unknown_fd_entry_returns_none() {
        // "42/fd/" (no number) should fail.
        assert_eq!(ProcFs::resolve_path("42/fd/"), None);
        // "42/fd/abc" (not a number) should fail.
        assert_eq!(ProcFs::resolve_path("42/fd/abc"), None);
    }

    #[test]
    fn test_fd_entry_inode_encoding_roundtrip() {
        let pid = 99;
        let fd_num: u64 = 5;
        let ino = PID_FD_OFFSET + pid + (fd_num << 32);
        let base = ino - PID_FD_OFFSET;
        assert_eq!(base & 0xFFFF_FFFF, pid);
        assert_eq!(base >> 32, fd_num);
    }

    #[test]
    fn test_stat_fd_directory() {
        let fs = ProcFs;
        // PID_FD_OFFSET + pid (no fd shift) is the fd directory.
        let pid = 1u64;
        let ino = PID_FD_OFFSET + pid;
        let meta = fs.stat(ino).unwrap();
        assert!(meta.is_dir);
        assert_eq!(meta.ino, ino);
    }

    #[test]
    fn test_stat_fd_entry() {
        let fs = ProcFs;
        // PID_FD_OFFSET + pid + (fd << 32) is an individual fd entry (not a directory).
        let pid = 1u64;
        let fd_num = 3u64;
        let ino = PID_FD_OFFSET + pid + (fd_num << 32);
        let meta = fs.stat(ino).unwrap();
        assert!(!meta.is_dir);
        assert_eq!(meta.ino, ino);
    }

    #[test]
    fn test_stat_environ() {
        let fs = ProcFs;
        let ino = PID_ENVIRON_OFFSET + 1;
        let meta = fs.stat(ino).unwrap();
        assert!(!meta.is_dir);
        assert_eq!(meta.ino, ino);
    }

    #[test]
    fn test_environ_format() {
        // Verify the null-separated format matches Linux convention.
        let mut env = alloc::collections::BTreeMap::new();
        env.insert(String::from("PATH"), String::from("/usr/bin"));
        env.insert(String::from("HOME"), String::from("/root"));
        let mut out = String::new();
        for (key, value) in &env {
            if !out.is_empty() {
                out.push('\0');
            }
            let _ = core::fmt::Write::write_fmt(&mut out, format_args!("{key}={value}"));
        }
        // BTreeMap iterates in sorted order: HOME, PATH.
        assert_eq!(out, "HOME=/root\0PATH=/usr/bin");
    }

    #[test]
    fn test_open_fd_path() {
        let fs = ProcFs;
        let ino = fs.open("42/fd/3", OpenFlags::READ).unwrap();
        assert_eq!(ino, PID_FD_OFFSET + 42 + (3u64 << 32));
        fs.close(ino).unwrap();
    }

    #[test]
    fn test_open_environ_path() {
        let fs = ProcFs;
        let ino = fs.open("42/environ", OpenFlags::READ).unwrap();
        assert_eq!(ino, PID_ENVIRON_OFFSET + 42);
        fs.close(ino).unwrap();
    }

    #[test]
    fn test_fd_directory_read_not_supported() {
        // Reading the fd directory inode (fd_num == 0) should return NotSupported.
        let fs = ProcFs;
        let pid = 1u64;
        let ino = PID_FD_OFFSET + pid;
        let mut buf = [0u8; 64];
        assert_eq!(fs.read(ino, 0, &mut buf), Err(FsError::NotSupported));
    }

    // ─── Additional content format tests ───

    #[test]
    fn test_resolve_with_leading_slash() {
        // Paths are trimmed of leading '/'.
        assert_eq!(ProcFs::resolve_path("/meminfo"), Some(MEMINFO_INO));
        assert_eq!(ProcFs::resolve_path("/net/tcp"), Some(NET_TCP_INO));
        assert_eq!(
            ProcFs::resolve_path("/42/status"),
            Some(PID_STATUS_OFFSET + 42)
        );
    }

    #[test]
    fn test_meminfo_content_has_all_fields() {
        let info = ProcFs::read_meminfo();
        assert!(info.starts_with("MemTotal:"));
        // Contains all expected labels.
        assert!(info.contains("MemTotal:"));
        assert!(info.contains("MemFree:"));
        assert!(info.contains("MemUsed:"));
        assert!(info.contains("FrameRegion:"));
        assert!(info.contains("FrameCount:"));
        assert!(info.contains("FrameSize:"));
        // Numeric values present.
        assert!(info.chars().any(|c| c.is_ascii_digit()));
        // Ends with a newline.
        assert!(info.ends_with('\n'));
    }

    #[test]
    fn test_cpuinfo_multiple_processors() {
        let info = ProcFs::read_cpuinfo();
        // Should have at least "processor" entries.
        let processor_count = info.matches("processor").count();
        assert!(processor_count >= 0);
        assert!(info.contains("vendor_id"));
        assert!(info.contains("model name"));
        // Each processor block should be separated by blank line.
        assert!(info.ends_with('\n'));
    }

    #[test]
    fn test_uptime_has_two_fields() {
        let uptime = ProcFs::read_uptime();
        // Format: "SECONDS.00 SECONDS.00\n"
        let parts: Vec<&str> = uptime.trim().split_whitespace().collect();
        assert_eq!(parts.len(), 2);
        // Both parts should be parseable as floats (or at least have a decimal).
        assert!(parts[0].contains('.'));
        assert!(parts[1].contains('.'));
        assert!(uptime.ends_with('\n'));
    }

    #[test]
    fn test_tcp_connection_empty() {
        // When no connections exist, should still show header.
        let tcp = ProcFs::read_tcp();
        assert!(tcp.contains("local_address"));
        assert!(tcp.contains("remote_address"));
        assert!(tcp.contains("state"));
    }

    #[test]
    fn test_udp_content_empty() {
        let udp = ProcFs::read_udp();
        assert!(udp.contains("local_address"));
        assert!(udp.contains("remote_address"));
        assert!(udp.contains("state"));
        // Empty listing should have a blank line after header.
        let lines: Vec<&str> = udp.lines().collect();
        assert_eq!(lines.len(), 2); // header + blank line (or empty line after header)
    }

    #[test]
    fn test_arp_format_with_entries() {
        let arp = ProcFs::read_arp();
        assert!(arp.contains("IP address"));
        assert!(arp.contains("HW type"));
        assert!(arp.contains("Flags"));
        assert!(arp.contains("HW address"));
    }

    #[test]
    fn test_dev_format() {
        let dev = ProcFs::read_dev();
        assert!(dev.contains("Inter-"));
        assert!(dev.contains("Receive"));
        assert!(dev.contains("Transmit"));
        assert!(dev.contains("eth0") || dev.contains("face"));
    }

    #[test]
    fn test_read_pid_cmdline_not_found() {
        let fs = ProcFs;
        // PID must be < 0x10000 to stay within the cmdline range (0x20000..0x30000).
        let pid = 50000u64;
        let ino = PID_CMDLINE_OFFSET + pid;
        let mut buf = [0u8; 64];
        assert_eq!(fs.read(ino, 0, &mut buf), Err(FsError::NotFound));
    }

    #[test]
    fn test_read_pid_status_not_found() {
        let fs = ProcFs;
        // PID must be < 0x10000 to stay within the status range (0x30000..0x40000).
        let pid = 50000u64;
        let ino = PID_STATUS_OFFSET + pid;
        let mut buf = [0u8; 64];
        assert_eq!(fs.read(ino, 0, &mut buf), Err(FsError::NotFound));
    }

    #[test]
    fn test_read_pid_maps_not_found() {
        let fs = ProcFs;
        // PID must be < 0x10000 to stay within the maps range (0x40000..0x50000).
        let pid = 50000u64;
        let ino = PID_MAPS_OFFSET + pid;
        let mut buf = [0u8; 64];
        assert_eq!(fs.read(ino, 0, &mut buf), Err(FsError::NotFound));
    }

    #[test]
    fn test_read_pid_environ_not_found() {
        let fs = ProcFs;
        // PID_ENVIRON_OFFSET = 0x60000. Use a pid that doesn't overflow into fd range.
        let pid = 50000u64;
        let ino = PID_ENVIRON_OFFSET + pid;
        let mut buf = [0u8; 64];
        assert_eq!(fs.read(ino, 0, &mut buf), Err(FsError::NotFound));
    }

    #[test]
    fn test_read_root_directory_not_supported() {
        let fs = ProcFs;
        let mut buf = [0u8; 64];
        assert_eq!(fs.read(ROOT_INO, 0, &mut buf), Err(FsError::NotSupported));
    }

    #[test]
    fn test_read_net_directory_not_supported() {
        let fs = ProcFs;
        let mut buf = [0u8; 64];
        assert_eq!(
            fs.read(NET_DIR_INO, 0, &mut buf),
            Err(FsError::NotSupported)
        );
    }

    #[test]
    fn test_read_pid_directory_not_supported() {
        let fs = ProcFs;
        let ino = PID_DIR_OFFSET + 1;
        let mut buf = [0u8; 64];
        assert_eq!(fs.read(ino, 0, &mut buf), Err(FsError::NotSupported));
    }

    #[test]
    fn test_stat_pid_directory() {
        let fs = ProcFs;
        let ino = PID_DIR_OFFSET + 42;
        let meta = fs.stat(ino).unwrap();
        assert!(meta.is_dir);
        assert!(!meta.is_symlink);
        assert_eq!(meta.ino, ino);
    }

    #[test]
    fn test_readdir_pid_directory_not_found() {
        let fs = ProcFs;
        let ino = PID_DIR_OFFSET + 99999;
        assert_eq!(fs.readdir(ino), Err(FsError::NotFound));
    }

    #[test]
    fn test_readdir_unknown_not_found() {
        let fs = ProcFs;
        assert_eq!(fs.readdir(NET_TCP_INO), Err(FsError::NotFound));
    }

    #[test]
    fn test_ifconfig_not_configured() {
        let ifcfg = ProcFs::read_ifconfig();
        // Should contain either "Status:" information.
        assert!(ifcfg.contains("Status:"));
    }

    #[test]
    fn test_read_version_offset_exact() {
        let fs = ProcFs;
        let mut buf = [0u8; 64];
        // Read from offset 0.
        let n = fs.read(VERSION_INO, 0, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"OpenOS 0.3.0\n");
        // Read partial from middle.
        let mut buf2 = [0u8; 8];
        let n2 = fs.read(VERSION_INO, 6, &mut buf2).unwrap();
        assert_eq!(&buf2[..n2], b" 0.3.0\n");
    }

    #[test]
    fn test_readdir_net_includes_all_entries() {
        let entries = ProcFs::readdir_net();
        let entry_names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(entry_names.contains(&"ifconfig"));
        assert!(entry_names.contains(&"tcp"));
        assert!(entry_names.contains(&"udp"));
        assert!(entry_names.contains(&"arp"));
        assert!(entry_names.contains(&"dev"));
    }

    #[test]
    fn test_readdir_root_includes_net_and_static() {
        let entries = ProcFs::readdir_root();
        let entry_names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(entry_names.contains(&"meminfo"));
        assert!(entry_names.contains(&"cpuinfo"));
        assert!(entry_names.contains(&"uptime"));
        assert!(entry_names.contains(&"version"));
        assert!(entry_names.contains(&"net"));
    }
}
