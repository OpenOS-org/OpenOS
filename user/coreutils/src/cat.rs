//! cat — concatenate and print files
//!
//! Usage: cat [file...]

#![no_std]
#![no_main]

mod common;

use common::{exit, stderrln, stdout_bytes};
use openos_sdk::fs;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // For now, read from /disk/test.txt as a demo
    let path = "/disk/test.txt";
    match fs::open(path) {
        Ok(fd) => {
            let mut buf = [0u8; 8192];
            loop {
                match fs::read(fd, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => stdout_bytes(&buf[..n]),
                    Err(_) => break,
                }
            }
            let _ = fs::close(fd);
        }
        Err(_) => {
            stderrln("cat: no such file");
        }
    }
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
