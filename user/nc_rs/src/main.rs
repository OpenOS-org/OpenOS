//! Netcat (nc) for OpenOS — TCP client and server tool.
//!
//! Supports two modes:
//! - **Client mode**: `nc <host> <port>` — connects to a remote host and
//!   relays data between stdin and the socket.
//! - **Listen mode**: `nc -l <port>` — listens on a local port, accepts one
//!   connection, and relays data between stdin and the socket.
//!
//! Demonstrates DNS resolution, TCP socket creation, connect/bind/listen/accept,
//! and bidirectional data transfer using the OpenOS SDK.

#![no_std]
#![no_main]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::panic::PanicInfo;

use openos_sdk::{console, dns, process, socket, thread};

/// Simple bump allocator for user-space (64 KiB heap).
struct BumpAllocator {
    heap: core::cell::UnsafeCell<[u8; 65536]>,
    offset: core::cell::Cell<usize>,
}

unsafe impl Sync for BumpAllocator {}

unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let align = layout.align();
        let size = layout.size();
        let mut off = self.offset.get();
        // Align
        off = (off + align - 1) & !(align - 1);
        if off + size > 65536 {
            return core::ptr::null_mut();
        }
        let ptr = (*self.heap.get()).as_mut_ptr().add(off);
        self.offset.set(off + size);
        ptr
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // Bump allocator: no-op dealloc.
    }
}

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator {
    heap: core::cell::UnsafeCell::new([0u8; 65536]),
    offset: core::cell::Cell::new(0),
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum bytes per send/recv operation.
const CHUNK_SIZE: usize = 1024;

/// Poll delay iterations between receive attempts (yield to let packets arrive).
const POLL_DELAY: usize = 100;

/// Maximum consecutive WouldBlock attempts on stdin before checking socket.
const STDIN_POLL_ATTEMPTS: usize = 50;

/// Maximum consecutive WouldBlock attempts on socket before checking stdin.
const SOCKET_POLL_ATTEMPTS: usize = 1000;

// ---------------------------------------------------------------------------
// Panic handler
// ---------------------------------------------------------------------------

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = console::writeln("PANIC in nc!");
    process::exit(1);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Format a usize as a decimal string into a static buffer.
fn format_usize(mut n: usize) -> &'static str {
    static mut BUF: [u8; 20] = [0u8; 20];
    if n == 0 {
        // SAFETY: single-threaded user program, no concurrent access.
        unsafe {
            BUF[19] = b'0';
            return core::str::from_utf8_unchecked(&BUF[19..20]);
        }
    }
    let mut i = 19;
    while n > 0 {
        // SAFETY: single-threaded user program, only ASCII digits written.
        unsafe {
            BUF[i] = b'0' + (n % 10) as u8;
        }
        n /= 10;
        i = i.saturating_sub(1);
    }
    // SAFETY: slice contains only ASCII digits.
    unsafe { core::str::from_utf8_unchecked(&BUF[i + 1..20]) }
}

/// Format an IPv4 address as a dotted-quad string into a static buffer.
fn format_ip(ip: &[u8; 4]) -> &'static str {
    static mut BUF: [u8; 16] = [0u8; 16];
    // SAFETY: single-threaded user program.
    unsafe {
        let mut pos = 0;
        for (idx, &octet) in ip.iter().enumerate() {
            if idx > 0 {
                BUF[pos] = b'.';
                pos += 1;
            }
            if octet >= 100 {
                BUF[pos] = b'0' + octet / 100;
                pos += 1;
            }
            if octet >= 10 {
                BUF[pos] = b'0' + (octet % 100) / 10;
                pos += 1;
            }
            BUF[pos] = b'0' + octet % 10;
            pos += 1;
        }
        core::str::from_utf8_unchecked(&BUF[..pos])
    }
}

/// Format an SDK error code into a static string.
fn format_error(e: openos_sdk::Error) -> &'static str {
    match e {
        openos_sdk::Error::InvalidArgument => "invalid argument",
        openos_sdk::Error::NotFound => "not found",
        openos_sdk::Error::PermissionDenied => "permission denied",
        openos_sdk::Error::OutOfMemory => "out of memory",
        openos_sdk::Error::Busy => "busy",
        openos_sdk::Error::ChannelClosed => "channel closed",
        openos_sdk::Error::WouldBlock => "would block",
        openos_sdk::Error::Timeout => "timeout",
        openos_sdk::Error::BadPointer => "bad pointer",
        openos_sdk::Error::UnknownSyscall => "unknown syscall",
        openos_sdk::Error::Unknown(_) => "unknown error",
    }
}

/// Yield the CPU briefly to let packets arrive or keyboard input appear.
fn poll_delay() {
    for _ in 0..POLL_DELAY {
        thread::yield_();
    }
}

// ---------------------------------------------------------------------------
// Argument parsing
// ---------------------------------------------------------------------------

/// Parsed command-line arguments.
enum Args<'a> {
    /// Connect to host:port.
    Client { host: &'a [u8], port: u16 },
    /// Listen on port.
    Listen { port: u16 },
}

/// Parse command-line arguments from a raw argument string.
///
/// Expected formats:
///   `nc <host> <port>`   — client mode
///   `nc -l <port>`       — listen mode
fn parse_args(raw: &str) -> Option<Args<'_>> {
    let mut parts = raw.split_whitespace();
    let first = parts.next()?;
    let second = parts.next()?;

    if first == "-l" {
        let port = second.parse::<u16>().ok()?;
        Some(Args::Listen { port })
    } else {
        let port = second.parse::<u16>().ok()?;
        Some(Args::Client {
            host: first.as_bytes(),
            port,
        })
    }
}

// ---------------------------------------------------------------------------
// DNS resolution
// ---------------------------------------------------------------------------

/// Resolve a hostname to an IPv4 address. If the host is already an IP
/// address literal, parse it directly.
fn resolve_host(host: &[u8]) -> Result<[u8; 4], openos_sdk::Error> {
    // Try to parse as a dotted-quad IP literal first.
    if let Ok(ip) = parse_ip_literal(host) {
        return Ok(ip);
    }

    // Fall back to DNS resolution.
    let host_str = core::str::from_utf8(host).map_err(|_| openos_sdk::Error::InvalidArgument)?;
    dns::resolve(host_str)
}

/// Parse an IPv4 dotted-quad literal (e.g., "10.0.2.2") from bytes.
fn parse_ip_literal(addr: &[u8]) -> Result<[u8; 4], ()> {
    let s = core::str::from_utf8(addr).map_err(|_| ())?;
    let mut octets = [0u8; 4];
    let mut idx = 0;
    for part in s.split('.') {
        if idx >= 4 {
            return Err(());
        }
        octets[idx] = part.parse::<u8>().map_err(|_| ())?;
        idx += 1;
    }
    if idx != 4 {
        return Err(());
    }
    Ok(octets)
}

// ---------------------------------------------------------------------------
// Bidirectional relay
// ---------------------------------------------------------------------------

/// Relay data bidirectionally between stdin/console and a TCP socket.
///
/// Reads from stdin (non-blocking console read) and sends to the socket.
/// Reads from the socket (non-blocking recv) and writes to stdout (console).
///
/// Runs indefinitely until the remote end closes the connection or an
/// unrecoverable error occurs.
fn relay(sock_fd: u64) {
    let mut recv_buf = [0u8; CHUNK_SIZE];
    let mut send_buf = [0u8; CHUNK_SIZE];
    let mut stdin_stale_count: usize = 0;
    let mut socket_stale_count: usize = 0;

    loop {
        // -- Try reading from stdin and sending to socket --
        match console::read(&mut send_buf, false) {
            Ok(0) => {
                // No data available from stdin right now.
                stdin_stale_count += 1;
                if stdin_stale_count >= STDIN_POLL_ATTEMPTS {
                    // Yield CPU and reset counter.
                    poll_delay();
                    stdin_stale_count = 0;
                }
            }
            Ok(n) => {
                stdin_stale_count = 0;
                if let Err(e) = socket::send(sock_fd, &send_buf[..n]) {
                    let _ = console::write("nc: send error: ");
                    let _ = console::writeln(format_error(e));
                    return;
                }
            }
            Err(openos_sdk::Error::WouldBlock) => {
                stdin_stale_count += 1;
                if stdin_stale_count >= STDIN_POLL_ATTEMPTS {
                    poll_delay();
                    stdin_stale_count = 0;
                }
            }
            Err(e) => {
                let _ = console::write("nc: stdin read error: ");
                let _ = console::writeln(format_error(e));
                return;
            }
        }

        // -- Try reading from socket and printing to stdout --
        match socket::recv(sock_fd, &mut recv_buf) {
            Ok(0) => {
                // Connection closed by remote.
                let _ = console::writeln("nc: connection closed by remote");
                return;
            }
            Ok(n) => {
                socket_stale_count = 0;
                // Print received bytes to console. Filter to printable ASCII
                // plus common whitespace, like curl_rs does.
                for &byte in &recv_buf[..n] {
                    if (0x20..0x7F).contains(&byte)
                        || byte == b'\n'
                        || byte == b'\r'
                        || byte == b'\t'
                    {
                        let single = [byte];
                        // SAFETY: single ASCII byte is valid UTF-8.
                        let ch = unsafe { core::str::from_utf8_unchecked(&single) };
                        let _ = console::write(ch);
                    }
                }
            }
            Err(openos_sdk::Error::WouldBlock) => {
                socket_stale_count += 1;
                if socket_stale_count >= SOCKET_POLL_ATTEMPTS {
                    poll_delay();
                    socket_stale_count = 0;
                }
            }
            Err(e) => {
                let _ = console::write("nc: recv error: ");
                let _ = console::writeln(format_error(e));
                return;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Read the command-line arguments. In OpenOS, the shell passes the
    // argument string as the first thing on stdin or via process creation.
    // Since OpenOS does not yet have argc/argv, we read a line from stdin
    // as the argument string.
    let _ = console::writeln("nc: reading arguments...");

    let mut arg_buf = [0u8; 256];
    let mut arg_len = 0;

    // Read the argument line (blocking until we get a newline or timeout).
    for _ in 0..100_000 {
        match console::read(&mut arg_buf[arg_len..], false) {
            Ok(0) => {
                thread::yield_();
            }
            Ok(n) => {
                arg_len += n;
                // Check if we got a newline.
                if arg_buf[..arg_len].contains(&b'\n') {
                    break;
                }
                if arg_len >= arg_buf.len() {
                    break;
                }
            }
            Err(openos_sdk::Error::WouldBlock) => {
                thread::yield_();
            }
            Err(_) => break,
        }
    }

    if arg_len == 0 {
        let _ = console::writeln("nc: no arguments provided");
        let _ = console::writeln("usage: nc <host> <port>");
        let _ = console::writeln("       nc -l <port>");
        process::exit(1);
    }

    let arg_str = core::str::from_utf8(&arg_buf[..arg_len]).unwrap_or("");
    let arg_str = arg_str.trim();

    let args = match parse_args(arg_str) {
        Some(a) => a,
        None => {
            let _ = console::writeln("nc: invalid arguments");
            let _ = console::writeln("usage: nc <host> <port>");
            let _ = console::writeln("       nc -l <port>");
            process::exit(1);
        }
    };

    match args {
        // -- Client mode --
        Args::Client { host, port } => {
            let _ = console::write("nc: connecting to ");
            let _ = console::write(core::str::from_utf8(host).unwrap_or("?"));
            let _ = console::write(":");
            let _ = console::write(format_usize(port as usize));
            let _ = console::writeln(" ...");

            // DNS resolution.
            let ip = match resolve_host(host) {
                Ok(addr) => {
                    let _ = console::write("nc: resolved -> ");
                    let _ = console::writeln(format_ip(&addr));
                    addr
                }
                Err(e) => {
                    let _ = console::write("nc: DNS resolution failed: ");
                    let _ = console::writeln(format_error(e));
                    process::exit(1);
                }
            };

            // Create TCP socket.
            let sock_fd = match socket::create_tcp() {
                Ok(fd) => fd,
                Err(e) => {
                    let _ = console::write("nc: socket creation failed: ");
                    let _ = console::writeln(format_error(e));
                    process::exit(1);
                }
            };

            // Connect.
            let addr_u32 = u32::from_be_bytes(ip);
            if let Err(e) = socket::connect(sock_fd, addr_u32, port) {
                let _ = console::write("nc: connect failed: ");
                let _ = console::writeln(format_error(e));
                let _ = socket::close(sock_fd);
                process::exit(1);
            }

            let _ = console::writeln("nc: connected!");

            // Relay data bidirectionally.
            relay(sock_fd);

            // Clean up.
            let _ = socket::close(sock_fd);
        }

        // -- Listen mode --
        Args::Listen { port } => {
            let _ = console::write("nc: listening on port ");
            let _ = console::write(format_usize(port as usize));
            let _ = console::writeln(" ...");

            // Create TCP socket.
            let listen_fd = match socket::create_tcp() {
                Ok(fd) => fd,
                Err(e) => {
                    let _ = console::write("nc: socket creation failed: ");
                    let _ = console::writeln(format_error(e));
                    process::exit(1);
                }
            };

            // Bind to the specified port.
            if let Err(e) = socket::bind(listen_fd, port) {
                let _ = console::write("nc: bind failed: ");
                let _ = console::writeln(format_error(e));
                let _ = socket::close(listen_fd);
                process::exit(1);
            }

            // Start listening.
            if let Err(e) = socket::listen(listen_fd) {
                let _ = console::write("nc: listen failed: ");
                let _ = console::writeln(format_error(e));
                let _ = socket::close(listen_fd);
                process::exit(1);
            }

            let _ = console::writeln("nc: waiting for incoming connection...");

            // Accept one connection.
            let conn_fd = match socket::accept(listen_fd) {
                Ok(fd) => {
                    let _ = console::writeln("nc: connection accepted!");
                    fd
                }
                Err(e) => {
                    let _ = console::write("nc: accept failed: ");
                    let _ = console::writeln(format_error(e));
                    let _ = socket::close(listen_fd);
                    process::exit(1);
                }
            };

            // Relay data bidirectionally on the accepted connection.
            relay(conn_fd);

            // Clean up.
            let _ = socket::close(conn_fd);
            let _ = socket::close(listen_fd);
        }
    }

    let _ = console::writeln("nc: done.");
    process::exit(0);
}
