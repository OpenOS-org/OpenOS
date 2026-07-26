//! rev — reverse lines characterwise
//!
//! Usage: rev [file]

#![no_std]
#![no_main]

mod common;

use common::{exit, stdout_bytes};
use openos_sdk::fs;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let path = "/disk/test.txt";

    match fs::open(path) {
        Ok(fd) => {
            let mut buf = [0u8; 8192];
            match fs::read(fd, &mut buf) {
                Ok(n) => {
                    let data = &buf[..n];
                    let mut start = 0;
                    for i in 0..data.len() {
                        if data[i] == b'\n' {
                            // Reverse the line
                            let line = &data[start..i];
                            for j in (0..line.len()).rev() {
                                stdout_bytes(&[line[j]]);
                            }
                            stdout_bytes(b"\n");
                            start = i + 1;
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
