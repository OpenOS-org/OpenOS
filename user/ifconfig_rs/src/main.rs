//! ifconfig — display network interface configuration
//!
//! Reads `/proc/net/ifconfig` from the kernel's procfs and prints the
//! network interface status including IP address, netmask, gateway,
//! MAC address, DHCP lease info, and interface statistics.
//!
//! Usage: ifconfig

#![no_std]
#![no_main]

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
