//! unexpand — convert spaces to tabs
//!
//! Usage: unexpand [file]

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
                    let mut space_count = 0;
                    for &b in data {
                        if b == b' ' {
                            space_count += 1;
                            col += 1;
                            if col % tab_stop == 0 && space_count > 1 {
                                stdout_bytes(b"\t");
                                space_count = 0;
                            }
                        } else {
                            // Flush remaining spaces
                            for _ in 0..space_count {
                                stdout_bytes(b" ");
                            }
                            space_count = 0;
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
