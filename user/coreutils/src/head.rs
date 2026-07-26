//! head — output the first part of files
//!
//! Usage: head [-n count] [file]

#![no_std]
#![no_main]

mod common;

use common::{exit, stdoutln};
use openos_sdk::fs;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let path = "/disk/test.txt";
    let max_lines = 10;

    match fs::open(path) {
        Ok(fd) => {
            let mut buf = [0u8; 8192];
            let mut line_count = 0;
            match fs::read(fd, &mut buf) {
                Ok(n) => {
                    let data = &buf[..n];
                    let mut start = 0;
                    for i in 0..data.len() {
                        if data[i] == b'\n' {
                            if line_count < max_lines {
                                if let Ok(line) = core::str::from_utf8(&data[start..i]) {
                                    stdoutln(line);
                                }
                            }
                            line_count += 1;
                            start = i + 1;
                        }
                    }
                    if line_count < max_lines && start < data.len() {
                        if let Ok(line) = core::str::from_utf8(&data[start..]) {
                            stdoutln(line);
                        }
                    }
                }
                Err(_) => {}
            }
            let _ = fs::close(fd);
        }
        Err(_) => {
            stdoutln("head: no such file");
        }
    }
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
