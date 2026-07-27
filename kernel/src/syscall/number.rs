//! System call numbers — single source of truth.
//!
//! Number assignments follow INTERFACE.md §Appendix A.
//! Both kernel and user-space must use these constants.

// ─── Channel (5) ───

/// Create a new IPC channel.
pub const SYS_CHANNEL_CREATE: u64 = 0x01;
/// Send a message on a channel.
pub const SYS_CHANNEL_SEND: u64 = 0x02;
/// Receive a message from a channel.
pub const SYS_CHANNEL_RECEIVE: u64 = 0x03;
/// Perform a synchronous call on a channel.
pub const SYS_CHANNEL_CALL: u64 = 0x04;
/// Reply to a channel call.
pub const SYS_CHANNEL_REPLY: u64 = 0x05;

// ─── Handle (3) ───

/// Close a handle.
pub const SYS_HANDLE_CLOSE: u64 = 0x10;
/// Duplicate a handle.
pub const SYS_HANDLE_DUPLICATE: u64 = 0x11;
/// Transfer a handle to another task.
pub const SYS_HANDLE_TRANSFER: u64 = 0x12;

// ─── Process (4) ───

/// Create a new process.
pub const SYS_PROCESS_CREATE: u64 = 0x30;
/// Start a process.
pub const SYS_PROCESS_START: u64 = 0x31;
/// Exit the current process.
pub const SYS_PROCESS_EXIT: u64 = 0x32;
/// Wait for a process to exit.
pub const SYS_PROCESS_WAIT: u64 = 0x33;
/// Set the program break (heap end).
pub const SYS_BRK: u64 = 0x34;
/// Memory map a region.
pub const SYS_MMAP: u64 = 0x35;
/// Unmap a memory region.
pub const SYS_MUNMAP: u64 = 0x36;
/// Get the current process ID.
pub const SYS_GETPID: u64 = 0x37;
/// Get the parent process ID.
pub const SYS_GETPPID: u64 = 0x38;
/// List all tasks with their info.
pub const SYS_LIST_TASKS: u64 = 0x3D;

// ─── Thread (3) ───

/// Create a new thread.
pub const SYS_THREAD_CREATE: u64 = 0x40;
/// Exit the current thread.
pub const SYS_THREAD_EXIT: u64 = 0x41;
/// Yield the current thread.
pub const SYS_THREAD_YIELD: u64 = 0x42;

// ─── Signal (4) ───

/// Send signal to process.
pub const SYS_KILL: u64 = 0x44;
/// Set signal handler.
pub const SYS_SIGNAL: u64 = 0x45;
/// Return from signal handler.
pub const SYS_SIGRETURN: u64 = 0x46;
/// Get/set signal mask.
pub const SYS_SIGPROCMASK: u64 = 0x4A;

// ─── Memory protection ───

/// Change memory protection on mmap'd pages.
pub const SYS_MPROTECT: u64 = 0x4B;

// ─── Poll ───

/// Multiplexed I/O readiness check.
pub const SYS_POLL: u64 = 0x4C;

// ─── Console (OpenOS-specific) ───

/// Write to the kernel debug console.
pub const SYS_CONSOLE_WRITE: u64 = 0xF0;
/// Read from the kernel debug console.
pub const SYS_CONSOLE_READ: u64 = 0xF4;

// ─── Sleep ───

/// Sleep for N timer ticks.
pub const SYS_SLEEP: u64 = 0xF1;

// ─── Event signaling ───

/// Create an event object.
pub const SYS_EVENT_CREATE: u64 = 0xF2;
/// Signal an event.
pub const SYS_EVENT_SIGNAL: u64 = 0xF3;
/// Wait for an event.
pub const SYS_EVENT_WAIT: u64 = 0xFB;
/// Destroy an event object.
pub const SYS_EVENT_DESTROY: u64 = 0xFC;

// ─── Service discovery ───

/// Register a service endpoint.
pub const SYS_ENDPOINT_REGISTER: u64 = 0xF5;
/// Discover a service endpoint.
pub const SYS_ENDPOINT_DISCOVER: u64 = 0xF6;

// ─── Filesystem ───

/// Open a file.
pub const SYS_FS_OPEN: u64 = 0xF7;
/// Read from a file.
pub const SYS_FS_READ: u64 = 0xF8;
/// Write to a file.
pub const SYS_FS_WRITE: u64 = 0xF9;
/// Close a file.
pub const SYS_FS_CLOSE: u64 = 0xFA;

// ─── Network ───

/// Send a network packet.
pub const SYS_NET_SEND: u64 = 0xFD;
/// Receive a network packet.
pub const SYS_NET_RECEIVE: u64 = 0xFE;

// ─── Socket abstraction ───

/// Create a socket.
pub const SYS_SOCKET: u64 = 0xA0;
/// Bind a socket to an address.
pub const SYS_BIND: u64 = 0xA1;
/// Listen for connections.
pub const SYS_LISTEN: u64 = 0xA2;
/// Accept a connection.
pub const SYS_ACCEPT: u64 = 0xA3;
/// Connect to a remote address.
pub const SYS_CONNECT: u64 = 0xA4;
/// Send data to a specific address.
pub const SYS_SENDTO: u64 = 0xA5;
/// Receive data from a specific address.
pub const SYS_RECVFROM: u64 = 0xA6;
/// Close a socket.
pub const SYS_CLOSE_SOCK: u64 = 0xA7;

// ─── DNS ───

/// Resolve a hostname to an IP address.
pub const SYS_DNS_RESOLVE: u64 = 0xA8;
/// Get a socket option.
pub const SYS_GETSOCKOPT: u64 = 0xA9;
/// Set a socket option.
pub const SYS_SETSOCKOPT: u64 = 0xAA;
/// Get the remote address and port of a connected socket.
pub const SYS_GETPEERNAME: u64 = 0xAB;
/// Get the local address and port of a bound socket.
pub const SYS_GETSOCKNAME: u64 = 0xAC;

// ─── Filesystem seek ───

/// Seek within a file.
pub const SYS_FS_SEEK: u64 = 0xFF;

// ─── Filesystem metadata ───

/// Delete a file.
pub const SYS_FS_UNLINK: u64 = 0xC0;
/// Rename a file.
pub const SYS_FS_RENAME: u64 = 0xC1;
/// Create a directory.
pub const SYS_FS_MKDIR: u64 = 0xC2;
/// Remove a directory.
pub const SYS_FS_RMDIR: u64 = 0xC3;
/// Get file status/metadata.
pub const SYS_FS_STAT: u64 = 0xC4;
/// Read directory entries.
pub const SYS_FS_READDIR: u64 = 0xC5;
/// Read directory entries in `linux_dirent64` format.
pub const SYS_GETDENTS64: u64 = 0xC6;
/// Get file status by file descriptor.
pub const SYS_FSTAT: u64 = 0xC7;
/// Get file status without following symlinks.
pub const SYS_LSTAT: u64 = 0xC8;
/// Check file accessibility.
pub const SYS_ACCESS: u64 = 0xC9;
/// Change file permissions.
pub const SYS_CHMOD: u64 = 0xCC;
/// Set and get the file mode creation mask.
pub const SYS_UMASK: u64 = 0xCF;
/// Create a symbolic link.
pub const SYS_SYMLINK: u64 = 0xCA;
/// Read symbolic link target.
pub const SYS_READLINK: u64 = 0xCB;

// ─── Seek whence constants ───

/// Seek from the beginning of the file.
pub const SEEK_SET: u64 = 0;
/// Seek from the current position.
pub const SEEK_CUR: u64 = 1;
/// Seek from the end of the file.
pub const SEEK_END: u64 = 2;

// ─── Access mode constants ───

/// Test for existence.
pub const F_OK: u64 = 0;
/// Test for read permission.
pub const R_OK: u64 = 1;
/// Test for write permission.
pub const W_OK: u64 = 2;
/// Test for execute permission.
pub const X_OK: u64 = 4;

// ─── Time ───

/// Get the current time for a clock.
pub const SYS_CLOCK_GETTIME: u64 = 0x3E;

// ─── Dup2 / Environment / Working directory ───

/// Duplicate file descriptor.
pub const SYS_DUP2: u64 = 0x47;
/// Get environment variable.
pub const SYS_ENV_GET: u64 = 0x48;
/// Set environment variable.
pub const SYS_ENV_SET: u64 = 0x49;
/// Change working directory.
pub const SYS_CHDIR: u64 = 0xCD;
/// Get working directory.
pub const SYS_GETCWD: u64 = 0xCE;

// ─── Hardware access (user-space driver support) ───

/// Read from an I/O port.
pub const SYS_PORT_IN: u64 = 0xB0;
/// Write to an I/O port.
pub const SYS_PORT_OUT: u64 = 0xB1;
/// Map MMIO memory.
pub const SYS_MMIO_MAP: u64 = 0xB2;
/// Unmap MMIO memory.
pub const SYS_MMIO_UNMAP: u64 = 0xB3;

// ─── IRQ ───

/// Wait for an IRQ.
pub const SYS_IRQ_WAIT: u64 = 0xB4;

// ─── Process group / session ───

/// Set the process group ID of a process.
pub const SYS_SETPGID: u64 = 0xD2;
/// Get the process group ID of a process.
pub const SYS_GETPGID: u64 = 0xD3;
/// Create a new session and set the session ID.
pub const SYS_SETSID: u64 = 0xD4;

// ─── UID / GID (4) ───

/// Get the real user ID.
pub const SYS_GETUID: u64 = 0xD5;
/// Get the real group ID.
pub const SYS_GETGID: u64 = 0xD6;
/// Set the real user ID.
pub const SYS_SETUID: u64 = 0xD7;
/// Set the real group ID.
pub const SYS_SETGID: u64 = 0xD8;

// ─── Shared memory ───
/// Get or create a shared memory segment.
pub const SYS_SHMGET: u64 = 0xD9;
/// Attach a shared memory segment.
pub const SYS_SHMAT: u64 = 0xDA;
/// Detach a shared memory segment.
pub const SYS_SHMDT: u64 = 0xDB;

// ─── Pipe ───

/// Create a pipe pair.
pub const SYS_PIPE: u64 = 0x43;

/// Scatter-gather read.
pub const SYS_READV: u64 = 0xE0;
/// Scatter-gather write.
pub const SYS_WRITEV: u64 = 0xE1;

// ─── Signal (extended) ───

/// Send signal to a specific thread in a process.
pub const SYS_TGKILL: u64 = 0xEC;
/// Real-time signal action (set/get signal handler with extended info).
pub const SYS_RT_SIGACTION: u64 = 0xED;
/// Real-time signal mask (alias for sigprocmask).
pub const SYS_RT_SIGPROCMASK: u64 = 0xEE;

// ─── Misc (0xE0-0xEF) ───

/// I/O control.
pub const SYS_IOCTL: u64 = 0xE6;
/// Get resource usage.
pub const SYS_GETRUSAGE: u64 = 0xE7;
/// Get/set resource limits.
pub const SYS_PRLIMIT: u64 = 0xE8;

// ─── Additional syscall numbers ───

/// Duplicate file descriptor with flags.
pub const SYS_DUP3: u64 = 0x4E;
/// Create an epoll instance.
pub const SYS_EPOLL_CREATE: u64 = 0x4F;
/// Control an epoll instance.
pub const SYS_EPOLL_CTL: u64 = 0x50;
/// Wait on an epoll instance.
pub const SYS_EPOLL_WAIT: u64 = 0x51;
/// Advise kernel about memory usage patterns.
pub const SYS_MADVISE: u64 = 0x52;
/// Drain one entry from the kernel syslog buffer.
pub const SYS_SYSLOG_DRAIN: u64 = 0xE2;
/// Create an interval timer.
pub const SYS_TIMER_CREATE: u64 = 0xD9;
/// Arm or disarm an interval timer.
pub const SYS_TIMER_SETTIME: u64 = 0xDA;
/// Query timer state.
pub const SYS_TIMER_GETTIME: u64 = 0xDB;
/// Apply or remove an advisory lock on a file.
pub const SYS_FLOCK: u64 = 0x53;
/// Get the current thread ID.
pub const SYS_GETTID: u64 = 0xEE;
/// Yield the CPU.
pub const SYS_SCHED_YIELD: u64 = 0xEF;
/// Fill a buffer with pseudo-random bytes.
pub const SYS_GETRANDOM: u64 = 0xF0;
/// Memory barrier.
pub const SYS_MEMBARRIER: u64 = 0xF2;

/// Create a named pipe (FIFO).
pub const SYS_MKFIFO: u64 = 0xDC;

mod tests {
    use super::*;

    // ─── Channel syscall numbers ───
    #[test]
    fn test_channel_numbers_sequential() {
        assert_eq!(SYS_CHANNEL_CREATE, 0x01);
        assert_eq!(SYS_CHANNEL_SEND, 0x02);
        assert_eq!(SYS_CHANNEL_RECEIVE, 0x03);
        assert_eq!(SYS_CHANNEL_CALL, 0x04);
        assert_eq!(SYS_CHANNEL_REPLY, 0x05);
    }

    // ─── Handle syscall numbers ───
    #[test]
    fn test_handle_numbers_sequential() {
        assert_eq!(SYS_HANDLE_CLOSE, 0x10);
        assert_eq!(SYS_HANDLE_DUPLICATE, 0x11);
        assert_eq!(SYS_HANDLE_TRANSFER, 0x12);
    }

    // ─── Process syscall numbers ───
    #[test]
    fn test_process_numbers_sequential() {
        assert_eq!(SYS_PROCESS_CREATE, 0x30);
        assert_eq!(SYS_PROCESS_START, 0x31);
        assert_eq!(SYS_PROCESS_EXIT, 0x32);
        assert_eq!(SYS_PROCESS_WAIT, 0x33);
        assert_eq!(SYS_GETPID, 0x37);
        assert_eq!(SYS_GETPPID, 0x38);
        assert_eq!(SYS_LIST_TASKS, 0x3D);
    }

    // ─── Thread syscall numbers ───
    #[test]
    fn test_thread_numbers_sequential() {
        assert_eq!(SYS_THREAD_CREATE, 0x40);
        assert_eq!(SYS_THREAD_EXIT, 0x41);
        assert_eq!(SYS_THREAD_YIELD, 0x42);
    }

    // ─── Console syscall number ───
    #[test]
    fn test_console_write_number() {
        assert_eq!(SYS_CONSOLE_WRITE, 0xF0);
    }

    // ─── Socket syscall numbers ───
    #[test]
    fn test_socket_numbers_sequential() {
        assert_eq!(SYS_SOCKET, 0xA0);
        assert_eq!(SYS_BIND, 0xA1);
        assert_eq!(SYS_LISTEN, 0xA2);
        assert_eq!(SYS_ACCEPT, 0xA3);
        assert_eq!(SYS_CONNECT, 0xA4);
        assert_eq!(SYS_SENDTO, 0xA5);
        assert_eq!(SYS_RECVFROM, 0xA6);
        assert_eq!(SYS_CLOSE_SOCK, 0xA7);
        assert_eq!(SYS_DNS_RESOLVE, 0xA8);
        assert_eq!(SYS_GETSOCKOPT, 0xA9);
        assert_eq!(SYS_SETSOCKOPT, 0xAA);
    }

    // ─── Dup2 / Environment / Working directory syscall numbers ───
    #[test]
    fn test_dup2_env_cwd_numbers() {
        assert_eq!(SYS_DUP2, 0x47);
        assert_eq!(SYS_ENV_GET, 0x48);
        assert_eq!(SYS_ENV_SET, 0x49);
        assert_eq!(SYS_CHDIR, 0xCD);
        assert_eq!(SYS_GETCWD, 0xCE);
    }

    // ─── Symlink syscall numbers ───
    #[test]
    fn test_symlink_numbers() {
        assert_eq!(SYS_SYMLINK, 0xCA);
        assert_eq!(SYS_READLINK, 0xCB);
    }

    // ─── fstat / lstat syscall numbers ───
    #[test]
    fn test_fstat_lstat_numbers() {
        assert_eq!(SYS_FSTAT, 0xC7);
        assert_eq!(SYS_LSTAT, 0xC8);
    }

    // ─── No overlaps between groups ───
    #[test]
    fn test_no_overlaps() {
        let all = [
            SYS_CHANNEL_CREATE,
            SYS_CHANNEL_SEND,
            SYS_CHANNEL_RECEIVE,
            SYS_CHANNEL_CALL,
            SYS_CHANNEL_REPLY,
            SYS_HANDLE_CLOSE,
            SYS_HANDLE_DUPLICATE,
            SYS_HANDLE_TRANSFER,
            SYS_PROCESS_CREATE,
            SYS_PROCESS_START,
            SYS_PROCESS_EXIT,
            SYS_PROCESS_WAIT,
            SYS_LIST_TASKS,
            SYS_THREAD_CREATE,
            SYS_THREAD_EXIT,
            SYS_THREAD_YIELD,
            SYS_KILL,
            SYS_SIGNAL,
            SYS_EVENT_CREATE,
            SYS_EVENT_SIGNAL,
            SYS_EVENT_WAIT,
            SYS_EVENT_DESTROY,
            SYS_CONSOLE_WRITE,
            SYS_CONSOLE_READ,
            SYS_FS_OPEN,
            SYS_FS_READ,
            SYS_FS_WRITE,
            SYS_FS_CLOSE,
            SYS_FS_SEEK,
            SYS_NET_SEND,
            SYS_NET_RECEIVE,
            SYS_SOCKET,
            SYS_BIND,
            SYS_LISTEN,
            SYS_ACCEPT,
            SYS_CONNECT,
            SYS_SENDTO,
            SYS_RECVFROM,
            SYS_CLOSE_SOCK,
            SYS_DNS_RESOLVE,
            SYS_GETSOCKOPT,
            SYS_SETSOCKOPT,
            SYS_PORT_IN,
            SYS_PORT_OUT,
            SYS_MMIO_MAP,
            SYS_MMIO_UNMAP,
            SYS_IRQ_WAIT,
            SYS_BRK,
            SYS_MMAP,
            SYS_MUNMAP,
            SYS_GETPID,
            SYS_GETPPID,
            SYS_CLOCK_GETTIME,
            SYS_PIPE,
            SYS_DUP2,
            SYS_ENV_GET,
            SYS_ENV_SET,
            SYS_CHDIR,
            SYS_GETCWD,
            SYS_FSTAT,
            SYS_LSTAT,
            SYS_ACCESS,
            SYS_SYMLINK,
            SYS_READLINK,
            SYS_CHMOD,
            SYS_UMASK,
            SYS_GETDENTS64,
            SYS_SETPGID,
            SYS_GETPGID,
            SYS_SETSID,
            SYS_MKFIFO,
            SYS_GETUID,
            SYS_GETGID,
            SYS_SETUID,
            SYS_SETGID,
            SYS_POLL,
            SYS_IOCTL,
            SYS_GETRUSAGE,
            SYS_PRLIMIT,
            SYS_TGKILL,
            SYS_RT_SIGACTION,
            SYS_RT_SIGPROCMASK,
        ];
        for i in 0..all.len() {
            for j in i + 1..all.len() {
                assert_ne!(
                    all[i], all[j],
                    "syscall numbers {} and {} overlap",
                    all[i], all[j]
                );
            }
        }
    }

    // ─── Channel group is in range 0x01-0x0F ───
    #[test]
    fn test_channel_range() {
        let channel_numbers = [
            SYS_CHANNEL_CREATE,
            SYS_CHANNEL_SEND,
            SYS_CHANNEL_RECEIVE,
            SYS_CHANNEL_CALL,
            SYS_CHANNEL_REPLY,
        ];
        for &n in &channel_numbers {
            assert!(n >= 0x01 && n <= 0x0F, "channel syscall {} out of range", n);
        }
    }

    // ─── Handle group is in range 0x10-0x1F ───
    #[test]
    fn test_handle_range() {
        let handle_numbers = [SYS_HANDLE_CLOSE, SYS_HANDLE_DUPLICATE, SYS_HANDLE_TRANSFER];
        for &n in &handle_numbers {
            assert!(n >= 0x10 && n <= 0x1F, "handle syscall {} out of range", n);
        }
    }

    // ─── Process group is in range 0x30-0x3F ───
    #[test]
    fn test_process_range() {
        let process_numbers = [
            SYS_PROCESS_CREATE,
            SYS_PROCESS_START,
            SYS_PROCESS_EXIT,
            SYS_PROCESS_WAIT,
            SYS_GETPID,
            SYS_GETPPID,
            SYS_LIST_TASKS,
        ];
        for &n in &process_numbers {
            assert!(n >= 0x30 && n <= 0x3F, "process syscall {} out of range", n);
        }
    }

    // ─── Thread group is in range 0x40-0x4F ───
    #[test]
    fn test_thread_range() {
        let thread_numbers = [SYS_THREAD_CREATE, SYS_THREAD_EXIT, SYS_THREAD_YIELD];
        for &n in &thread_numbers {
            assert!(n >= 0x40 && n <= 0x4F, "thread syscall {} out of range", n);
        }
    }

    // ─── Signal syscall numbers ───
    #[test]
    fn test_signal_numbers() {
        assert_eq!(SYS_KILL, 0x44);
        assert_eq!(SYS_SIGNAL, 0x45);
    }

    // ─── Extended signal syscall numbers ───
    #[test]
    fn test_extended_signal_numbers() {
        assert_eq!(SYS_TGKILL, 0xEC);
        assert_eq!(SYS_RT_SIGACTION, 0xED);
        assert_eq!(SYS_RT_SIGPROCMASK, 0xEE);
    }

    // ─── Hardware access syscall numbers ───
    #[test]
    fn test_hardware_access_numbers() {
        assert_eq!(SYS_PORT_IN, 0xB0);
        assert_eq!(SYS_PORT_OUT, 0xB1);
        assert_eq!(SYS_MMIO_MAP, 0xB2);
        assert_eq!(SYS_MMIO_UNMAP, 0xB3);
        assert_eq!(SYS_IRQ_WAIT, 0xB4);
    }

    // ─── Process group / session syscall numbers ───
    #[test]
    fn test_process_group_session_numbers() {
        assert_eq!(SYS_SETPGID, 0xD2);
        assert_eq!(SYS_GETPGID, 0xD3);
        assert_eq!(SYS_SETSID, 0xD4);
    }

    // ─── UID/GID syscall numbers ───
    #[test]
    fn test_uid_gid_numbers() {
        assert_eq!(SYS_GETUID, 0xD5);
        assert_eq!(SYS_GETGID, 0xD6);
        assert_eq!(SYS_SETUID, 0xD7);
        assert_eq!(SYS_SETGID, 0xD8);
    }

    // ─── UID/GID numbers are in process group range (0xD0-0xDF) ───
    #[test]
    fn test_uid_gid_in_process_group_range() {
        let uid_gid_numbers = [SYS_GETUID, SYS_GETGID, SYS_SETUID, SYS_SETGID];
        for &n in &uid_gid_numbers {
            assert!(
                n >= 0xD0 && n <= 0xDF,
                "uid/gid syscall {} out of process group range",
                n
            );
        }
    }

    // ─── Pipe syscall number ───
    #[test]
    fn test_pipe_number() {
        assert_eq!(SYS_PIPE, 0x43);
    }

    // ─── Time syscall number ───
    #[test]
    fn test_clock_gettime_number() {
        assert_eq!(SYS_CLOCK_GETTIME, 0x3E);
    }

    // ─── Time syscall is in process range ───
    #[test]
    fn test_clock_gettime_in_process_range() {
        assert!(
            SYS_CLOCK_GETTIME >= 0x30 && SYS_CLOCK_GETTIME <= 0x3F,
            "clock_gettime syscall {} out of process range",
            SYS_CLOCK_GETTIME
        );
    }

    // ─── Access syscall number ───
    #[test]
    fn test_access_number() {
        assert_eq!(SYS_ACCESS, 0xC9);
    }

    // ─── Access syscall is in filesystem metadata range ───
    #[test]
    fn test_access_in_fs_metadata_range() {
        assert!(
            SYS_ACCESS >= 0xC0 && SYS_ACCESS <= 0xCF,
            "access syscall {} out of filesystem metadata range",
            SYS_ACCESS
        );
    }

    // ─── Chmod / Umask syscall numbers ───
    #[test]
    fn test_chmod_umask_numbers() {
        assert_eq!(SYS_CHMOD, 0xCC);
        assert_eq!(SYS_UMASK, 0xCF);
    }

    // ─── Chmod / Umask are in filesystem metadata range ───
    #[test]
    fn test_chmod_umask_in_fs_metadata_range() {
        assert!(
            SYS_CHMOD >= 0xC0 && SYS_CHMOD <= 0xCF,
            "chmod syscall {} out of filesystem metadata range",
            SYS_CHMOD
        );
        assert!(
            SYS_UMASK >= 0xC0 && SYS_UMASK <= 0xCF,
            "umask syscall {} out of filesystem metadata range",
            SYS_UMASK
        );
    }

    // ─── Seek whence constants ───
    #[test]
    fn test_seek_constants() {
        assert_eq!(SEEK_SET, 0);
        assert_eq!(SEEK_CUR, 1);
        assert_eq!(SEEK_END, 2);
    }

    // ─── Access mode constants ───
    #[test]
    fn test_access_mode_constants() {
        assert_eq!(F_OK, 0);
        assert_eq!(R_OK, 1);
        assert_eq!(W_OK, 2);
        assert_eq!(X_OK, 4);
    }

    // ─── Access modes are powers of two (except F_OK) ───
    #[test]
    fn test_access_modes_are_flags() {
        // F_OK = 0 (existence check), others are bit flags.
        assert_eq!(R_OK.count_ones(), 1);
        assert_eq!(W_OK.count_ones(), 1);
        assert_eq!(X_OK.count_ones(), 1);
        // Combined flags should not overlap.
        assert_eq!(R_OK & W_OK, 0);
        assert_eq!(R_OK & X_OK, 0);
        assert_eq!(W_OK & X_OK, 0);
    }

    // ─── Poll syscall number ───
    #[test]
    fn test_poll_number() {
        assert_eq!(SYS_POLL, 0x4C);
    }

    // ─── Poll is in thread range (0x40-0x4F) ───
    #[test]
    fn test_poll_in_thread_range() {
        assert!(
            SYS_POLL >= 0x40 && SYS_POLL <= 0x4F,
            "poll syscall {} out of thread range",
            SYS_POLL
        );
    }
}
