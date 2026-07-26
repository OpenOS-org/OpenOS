//! file — determine file type
//!
//! Usage: file [file...]

#![no_std]
#![no_main]

mod common;

use common::{exit, stdout, stdoutln};
use openos_sdk::fs;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let path = "/disk/test.txt";

    match fs::open(path) {
        Ok(fd) => {
            let mut buf = [0u8; 64];
            match fs::read(fd, &mut buf) {
                Ok(n) => {
                    let data = &buf[..n];
                    stdout(path);
                    if data.starts_with(b"\x7fELF") {
                        stdoutln(": ELF 64-bit executable");
                    } else if data.starts_with(b"OSRD") {
                        stdoutln(": OpenOS initrd archive");
                    } else if data.starts_with(b"{") {
                        stdoutln(": JSON data");
                    } else if data.starts_with(b"#!") {
                        stdoutln(": script");
                    } else {
                        stdoutln(": ASCII text");
                    }
                }
                Err(_) => {}
            }
            let _ = fs::close(fd);
        }
        Err(_) => {
            stdout(path);
            stdoutln(": cannot open");
        }
    }
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
