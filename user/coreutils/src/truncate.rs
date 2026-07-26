//! truncate — shrink or extend files to a specified size
//!
//! Usage: truncate -s SIZE FILE
//!
//! If the file is larger, it is truncated to SIZE bytes.
//! If smaller, it is extended with zero bytes.

#![no_std]
#![no_main]

mod common;

use common::{exit, stderrln, stdoutln};
use openos_sdk::fs;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Demo: truncate /disk/test.txt to 100 bytes
    // In a real shell, -s SIZE and FILE come from argv.
    let target_size: u64 = 100;
    let path = "/disk/test.txt";

    let current_size = match fs::file_size(path) {
        Ok(sz) => sz,
        Err(_) => {
            stderrln("truncate: cannot stat file");
            exit(1);
        }
    };

    if target_size > current_size {
        // Extend: open and write zero bytes
        let fd = match fs::open(path) {
            Ok(fd) => fd,
            Err(_) => {
                // File does not exist; create it
                match fs::create(path) {
                    Ok(fd) => fd,
                    Err(_) => {
                        stderrln("truncate: cannot create file");
                        exit(1);
                    }
                }
            }
        };
        // Seek to end
        let _ = fs::seek(fd, 0, 2);
        // Write zeros to extend
        let zeros = [0u8; 256];
        let mut remaining = target_size - current_size;
        while remaining > 0 {
            let chunk = if remaining > 256 {
                256
            } else {
                remaining as usize
            };
            match fs::write(fd, &zeros[..chunk]) {
                Ok(n) => remaining -= n as u64,
                Err(_) => break,
            }
        }
        let _ = fs::close(fd);
    } else if target_size < current_size {
        // Shrink: read content, truncate, rewrite first target_size bytes
        let fd = match fs::open(path) {
            Ok(fd) => fd,
            Err(_) => {
                stderrln("truncate: cannot open file");
                exit(1);
            }
        };

        let mut buf = [0u8; 8192];
        let n = fs::read(fd, &mut buf).unwrap_or(0);
        let _ = fs::close(fd);

        // Recreate with truncated content
        let fd = match fs::create(path) {
            Ok(fd) => fd,
            Err(_) => {
                stderrln("truncate: cannot rewrite file");
                exit(1);
            }
        };

        let write_len = core::cmp::min(target_size as usize, n);
        let _ = fs::write(fd, &buf[..write_len]);
        let _ = fs::close(fd);
    }
    // If equal, nothing to do.

    stdoutln("truncate: done");
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
