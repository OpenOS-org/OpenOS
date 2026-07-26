//! expand — convert tabs to spaces
//!
//! Usage: expand [file]

#![no_std]
#![no_main]

mod common;

use common::{exit, stdout_bytes};
use openos_sdk::fs;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let path = "/disk/test.txt";
    let tab_stop = 8;

    match fs::open(path) {
        Ok(fd) => {
            let mut buf = [0u8; 8192];
            match fs::read(fd, &mut buf) {
                Ok(n) => {
                    let data = &buf[..n];
                    let mut col = 0;
                    for &b in data {
                        if b == b'\t' {
                            let spaces = tab_stop - (col % tab_stop);
                            for _ in 0..spaces {
                                stdout_bytes(b" ");
                                col += 1;
                            }
                        } else {
                            stdout_bytes(&[b]);
                            if b == b'\n' {
                                col = 0;
                            } else {
                                col += 1;
                            }
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
