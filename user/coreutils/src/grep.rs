//! grep — search for patterns in files
//!
//! Usage: grep pattern [file]

#![no_std]
#![no_main]

mod common;

use common::{exit, stdoutln};
use openos_sdk::fs;

/// Simple substring search in a line.
fn contains_pattern(line: &[u8], pattern: &[u8]) -> bool {
    if pattern.is_empty() {
        return true;
    }
    line.windows(pattern.len()).any(|w| w == pattern)
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Demo: search for "OpenOS" in /disk/test.txt
    let pattern = b"OpenOS";
    let path = "/disk/test.txt";

    match fs::open(path) {
        Ok(fd) => {
            let mut buf = [0u8; 8192];
            match fs::read(fd, &mut buf) {
                Ok(n) => {
                    let data = &buf[..n];
                    let mut start = 0;
                    for i in 0..data.len() {
                        if data[i] == b'\n' {
                            let line = &data[start..i];
                            if contains_pattern(line, pattern) {
                                if let Ok(s) = core::str::from_utf8(line) {
                                    stdoutln(s);
                                }
                            }
                            start = i + 1;
                        }
                    }
                    // Last line (no trailing newline)
                    if start < data.len() {
                        let line = &data[start..];
                        if contains_pattern(line, pattern) {
                            if let Ok(s) = core::str::from_utf8(line) {
                                stdoutln(s);
                            }
                        }
                    }
                }
                Err(_) => {}
            }
            let _ = fs::close(fd);
        }
        Err(_) => {
            stdoutln("grep: no such file");
        }
    }
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
