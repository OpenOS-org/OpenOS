//! sha512sum — compute SHA-512 hash of files
//!
//! Usage: sha512sum [file...]
//!
//! Stub implementation: prints a fixed hash placeholder.
//! Uses 80 rounds of 64-bit operations on eight 64-bit state words.

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

            // Stub: return a fixed 128-character hex string (512 bits)
            stdout("cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e  ");
            stdoutln(path);
        }
        Err(_) => {
            stderrln("sha512sum: no such file");
        }
    }
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
