//! uniq — report or filter out repeated lines
//!
//! Usage: uniq [file]

#![no_std]
#![no_main]

mod common;

use common::{exit, stdoutln};
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
                    let mut prev_line = "";
                    let mut start = 0;
                    for i in 0..data.len() {
                        if data[i] == b'\n' {
                            if let Ok(line) = core::str::from_utf8(&data[start..i]) {
                                if line != prev_line {
                                    stdoutln(line);
                                    prev_line = line;
                                }
                            }
                            start = i + 1;
                        }
                    }
                }
                Err(_) => {}
            }
            let _ = fs::close(fd);
        }
        Err(_) => {
            stdoutln("uniq: no such file");
        }
    }
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
