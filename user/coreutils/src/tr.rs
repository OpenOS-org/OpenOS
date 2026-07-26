//! tr — translate or delete characters
//!
//! Usage: tr [-d] set1 [set2]

#![no_std]
#![no_main]

mod common;

use common::{exit, stdout_bytes};
use openos_sdk::fs;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Demo: convert lowercase to uppercase
    let path = "/disk/test.txt";

    match fs::open(path) {
        Ok(fd) => {
            let mut buf = [0u8; 8192];
            match fs::read(fd, &mut buf) {
                Ok(n) => {
                    let data = &buf[..n];
                    for &b in data {
                        if b >= b'a' && b <= b'z' {
                            stdout_bytes(&[b - 32]); // to uppercase
                        } else {
                            stdout_bytes(&[b]);
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
