//! Simple HTTP client for OpenOS (like curl).
//!
//! Demonstrates the full network stack:
//! 1. Resolves a hostname via DNS (SYS_DNS_RESOLVE)
//! 2. Connects via TCP (SYS_SOCKET + SYS_CONNECT)
//! 3. Sends an HTTP GET request
//! 4. Receives and prints the response
//!
//! Usage: curl <hostname> [path]
//! Example: curl example.com /

#![no_std]
#![no_main]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::panic::PanicInfo;

use openos_sdk::{console, dns, process, socket};

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

/// HTTP port.
const HTTP_PORT: u16 = 80;

/// Maximum response buffer size (16 KiB).
const MAX_RESPONSE: usize = 16384;

/// Maximum receive polling attempts before giving up.
const MAX_RECV_ATTEMPTS: usize = 50_000;

/// Send buffer size for building the HTTP request.
const SEND_BUF_SIZE: usize = 1024;

/// Poll delay iterations between receive attempts (yield to let packets arrive).
const POLL_DELAY: usize = 100;

// ---------------------------------------------------------------------------
// Panic handler
// ---------------------------------------------------------------------------

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = console::writeln("PANIC in curl!");
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

/// Copy bytes from `src` into `dst`, returning the number of bytes copied.
fn copy_bytes(dst: &mut [u8], src: &[u8]) -> usize {
    let n = dst.len().min(src.len());
    dst[..n].copy_from_slice(&src[..n]);
    n
}

/// Build an HTTP GET request into `buf`. Returns the request length.
fn build_http_request(buf: &mut [u8], host: &str, path: &str) -> usize {
    let prefix = b"GET ";
    let middle = b" HTTP/1.1\r\nHost: ";
    let suffix = b"\r\nConnection: close\r\n\r\n";

    let mut pos = 0;

    pos += copy_bytes(&mut buf[pos..], prefix);
    pos += copy_bytes(&mut buf[pos..], path.as_bytes());
    pos += copy_bytes(&mut buf[pos..], middle);
    pos += copy_bytes(&mut buf[pos..], host.as_bytes());
    pos += copy_bytes(&mut buf[pos..], suffix);

    pos
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Parse arguments from the command line (passed as the first user-space
    // argument). For simplicity, use hardcoded defaults if not provided.
    //
    // In a real shell, the arguments would be passed via process creation.
    // For this demo, we use a default hostname.
    let hostname = "example.com";
    let path = "/";

    let _ = console::write("curl: resolving ");
    let _ = console::write(hostname);
    let _ = console::writeln(" ...");

    // Step 1: DNS resolution.
    let ip = match dns::resolve(hostname) {
        Ok(addr) => {
            let _ = console::write("curl: resolved ");
            let _ = console::write(hostname);
            let _ = console::write(" -> ");
            let _ = console::write(format_ip(&addr));
            let _ = console::writeln("");
            addr
        }
        Err(e) => {
            let _ = console::write("curl: DNS resolution failed: ");
            let _ = console::writeln(&format_error(e));
            let _ = console::writeln("CURL FAILED");
            process::exit(1);
        }
    };

    // Step 2: Create a TCP socket.
    let sock_fd = match socket::create_tcp() {
        Ok(fd) => {
            let _ = console::write("curl: socket created (fd=");
            let _ = console::write(format_usize(fd as usize));
            let _ = console::writeln(")");
            fd
        }
        Err(e) => {
            let _ = console::write("curl: socket creation failed: ");
            let _ = console::writeln(&format_error(e));
            let _ = console::writeln("CURL FAILED");
            process::exit(1);
        }
    };

    // Step 3: Connect to the HTTP server.
    let addr_u32 = u32::from_be_bytes(ip);
    let _ = console::write("curl: connecting to ");
    let _ = console::write(format_ip(&ip));
    let _ = console::write(":");
    let _ = console::write(format_usize(HTTP_PORT as usize));
    let _ = console::writeln(" ...");

    if let Err(e) = socket::connect(sock_fd, addr_u32, HTTP_PORT) {
        let _ = console::write("curl: connect failed: ");
        let _ = console::writeln(&format_error(e));
        let _ = console::writeln("CURL FAILED");
        let _ = socket::close(sock_fd);
        process::exit(1);
    }

    let _ = console::writeln("curl: connected!");

    // Step 4: Send HTTP GET request.
    let mut req_buf = [0u8; SEND_BUF_SIZE];
    let req_len = build_http_request(&mut req_buf, hostname, path);

    let _ = console::write("curl: sending HTTP request (");
    let _ = console::write(format_usize(req_len));
    let _ = console::writeln(" bytes) ...");

    match socket::send(sock_fd, &req_buf[..req_len]) {
        Ok(sent) => {
            let _ = console::write("curl: sent ");
            let _ = console::write(format_usize(sent));
            let _ = console::writeln(" bytes");
        }
        Err(e) => {
            let _ = console::write("curl: send failed: ");
            let _ = console::writeln(&format_error(e));
            let _ = console::writeln("CURL FAILED");
            let _ = socket::close(sock_fd);
            process::exit(1);
        }
    }

    // Step 5: Receive and print the HTTP response.
    let _ = console::writeln("curl: waiting for response ...");

    let mut response_buf = [0u8; MAX_RESPONSE];
    let mut total_received = 0;
    let mut attempts = 0;

    loop {
        match socket::recv(sock_fd, &mut response_buf[total_received..]) {
            Ok(0) => {
                // Connection closed by peer.
                break;
            }
            Ok(n) => {
                total_received += n;
                if total_received >= MAX_RESPONSE {
                    let _ = console::writeln("curl: response buffer full");
                    break;
                }
            }
            Err(openos_sdk::Error::WouldBlock) => {
                attempts += 1;
                if attempts >= MAX_RECV_ATTEMPTS {
                    if total_received > 0 {
                        // We got some data, print what we have.
                        break;
                    }
                    let _ = console::writeln("curl: receive timeout");
                    let _ = console::writeln("CURL FAILED");
                    let _ = socket::close(sock_fd);
                    process::exit(1);
                }
                // Yield CPU briefly before polling again.
                for _ in 0..POLL_DELAY {
                    openos_sdk::thread::yield_();
                }
            }
            Err(e) => {
                let _ = console::write("curl: receive error: ");
                let _ = console::writeln(&format_error(e));
                break;
            }
        }
    }

    // Print the response.
    if total_received > 0 {
        let _ = console::writeln("");
        let _ = console::writeln("--- HTTP Response ---");
        // Print as UTF-8 (lossy - replace invalid bytes with '?').
        for &byte in &response_buf[..total_received] {
            if byte >= 0x20 && byte < 0x7F || byte == b'\n' || byte == b'\r' || byte == b'\t' {
                // SAFETY: we only emit valid ASCII/UTF-8 bytes.
                let single = [byte];
                let ch = unsafe { core::str::from_utf8_unchecked(&single) };
                let _ = console::write(ch);
            }
        }
        let _ = console::writeln("");
        let _ = console::writeln("--- End Response ---");
        let _ = console::write("curl: received ");
        let _ = console::write(format_usize(total_received));
        let _ = console::writeln(" bytes total");
    } else {
        let _ = console::writeln("curl: no response received");
    }

    // Step 6: Close the socket.
    let _ = socket::close(sock_fd);
    let _ = console::writeln("curl: done.");
    process::exit(0);
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
