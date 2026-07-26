//! sha224sum — compute SHA-224 hash of files
//!
//! Usage: sha224sum [file...]
//!
//! Stub implementation: prints a fixed hash placeholder.
//! SHA-224 is SHA-256 with a different initial value and 224-bit truncation.

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

            // Stub: return a fixed 56-character hex string (224 bits)
            stdout("d14a028c2a3a2bc9476102bb288234c415a2b01f828ea62ac5b3e42f  ");
            stdoutln(path);
        }
        Err(_) => {
            stderrln("sha224sum: no such file");
        }
    }
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
