//! ifconfig — display network interface configuration
//!
//! Reads `/proc/net/ifconfig` from the kernel's procfs and prints the
//! network interface status including IP address, netmask, gateway,
//! MAC address, DHCP lease info, and interface statistics.
//!
//! Usage: ifconfig

#![no_std]
#![no_main]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::panic::PanicInfo;

use openos_sdk::{console, fs, process};

/// Buffer size for reading /proc/net/ifconfig content.
const READ_BUF_SIZE: usize = 1024;

/// Path to the network interface configuration procfs file.
const PROC_NET_IFCONFIG: &str = "/proc/net/ifconfig";

// ---------------------------------------------------------------------------
// Panic handler
// ---------------------------------------------------------------------------

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = console::writeln("PANIC in ifconfig!");
    process::exit(1);
}

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
// Helpers
// ---------------------------------------------------------------------------

/// Write a string to stdout (console).
fn stdout(s: &str) {
    let _ = console::write(s);
}

/// Write a string followed by newline to stdout.
fn stdoutln(s: &str) {
    let _ = console::writeln(s);
}

/// Write bytes to stdout, interpreting as UTF-8.
fn stdout_bytes(data: &[u8]) {
    if let Ok(s) = core::str::from_utf8(data) {
        let _ = console::write(s);
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Open /proc/net/ifconfig for reading.
    let fd = match fs::open(PROC_NET_IFCONFIG) {
        Ok(fd) => fd,
        Err(_) => {
            stdoutln("ifconfig: cannot open /proc/net/ifconfig");
            stdoutln("ifconfig: network may not be initialized");
            process::exit(1);
        }
    };

    // Read and display the file content.
    let mut buf = [0u8; READ_BUF_SIZE];
    loop {
        match fs::read(fd, &mut buf) {
            Ok(0) => break, // EOF
            Ok(n) => stdout_bytes(&buf[..n]),
            Err(_) => {
                stdoutln("ifconfig: error reading /proc/net/ifconfig");
                let _ = fs::close(fd);
                process::exit(1);
            }
        }
    }

    // Ensure we end with a newline for clean output.
    stdout("\n");

    let _ = fs::close(fd);
    process::exit(0);
}
