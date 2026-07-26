//! sha384sum — compute SHA-384 hash of files
//!
//! Usage: sha384sum [file...]
//!
//! Stub implementation: prints a fixed hash placeholder.
//! SHA-384 is SHA-512 with a different initial value and 384-bit truncation.

#![no_std]
#![no_main]

mod common;

use common::{exit, stderrln, stdout, stdoutln};
use openos_sdk::fs;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let path = "/disk/test.txt";

    match fs::open(path) {
        Ok(fd) => {
            // Read the file to report its name (stub: fixed output)
            let mut buf = [0u8; 4096];
            loop {
                match fs::read(fd, &mut buf) {
                    Ok(0) => break,
                    Ok(_) => continue,
                    Err(_) => break,
                }
            }
            let _ = fs::close(fd);

            // Stub: return a fixed 96-character hex string (384 bits)
            stdout("38b060a751ac96384cd9327eb1b1e36a21fdb71114be07434c0cc7bf63f6e1da274edebfe76f65fbd51ad2f14898b95b  ");
            stdoutln(path);
        }
        Err(_) => {
            stderrln("sha384sum: no such file");
        }
    }
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
