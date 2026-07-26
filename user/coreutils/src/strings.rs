//! strings — find printable strings in files
//!
//! Usage: strings [file]

#![no_std]
#![no_main]

mod common;

use common::{exit, stdout_bytes, stdout_bytes as stderr_bytes};
use openos_sdk::fs;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let path = "/disk/test.txt";
    let min_len = 4;

    match fs::open(path) {
        Ok(fd) => {
            let mut buf = [0u8; 4096];
            match fs::read(fd, &mut buf) {
                Ok(n) => {
                    let data = &buf[..n];
                    let mut current: [u8; 256] = [0; 256];
                    let mut current_len = 0;
                    for &b in data {
                        if b >= 0x20 && b < 0x7F {
                            if current_len < 256 {
                                current[current_len] = b;
                                current_len += 1;
                            }
                        } else {
                            if current_len >= min_len {
                                stdout_bytes(&current[..current_len]);
                                stdout_bytes(b"\n");
                            }
                            current_len = 0;
                        }
                    }
                    if current_len >= min_len {
                        stdout_bytes(&current[..current_len]);
                        stdout_bytes(b"\n");
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
