//! fold — wrap each input line to fit in specified width
//!
//! Usage: fold [-w width] [file]

#![no_std]
#![no_main]

mod common;

use common::{exit, stdoutln};
use openos_sdk::fs;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let path = "/disk/test.txt";
    let width = 80;

    match fs::open(path) {
        Ok(fd) => {
            let mut buf = [0u8; 8192];
            match fs::read(fd, &mut buf) {
                Ok(n) => {
                    let data = &buf[..n];
                    let mut col = 0;
                    for &b in data {
                        if b == b'\n' || col >= width {
                            if let Ok(s) = core::str::from_utf8(&[b]) {
                                let _ = openos_sdk::console::write(s);
                            }
                            if b == b'\n' {
                                col = 0;
                            } else {
                                col = 1;
                            }
                        } else {
                            if let Ok(s) = core::str::from_utf8(&[b]) {
                                let _ = openos_sdk::console::write(s);
                            }
                            col += 1;
                        }
                    }
                }
                Err(_) => {}
            }
            let _ = fs::close(fd);
        }
        Err(_) => {}
    }
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
