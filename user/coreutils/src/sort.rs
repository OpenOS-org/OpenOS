//! sort — sort lines of text
//!
//! Usage: sort [file]

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
                    // Collect lines
                    let mut lines: [&str; 256] = [""; 256];
                    let mut count = 0;
                    let mut start = 0;
                    for i in 0..data.len() {
                        if data[i] == b'\n' {
                            if count < 256 {
                                if let Ok(line) = core::str::from_utf8(&data[start..i]) {
                                    lines[count] = line;
                                    count += 1;
                                }
                            }
                            start = i + 1;
                        }
                    }
                    // Simple insertion sort (good enough for small lists)
                    for i in 1..count {
                        let key = lines[i];
                        let mut j = i;
                        while j > 0 && lines[j - 1] > key {
                            lines[j] = lines[j - 1];
                            j -= 1;
                        }
                        lines[j] = key;
                    }
                    for i in 0..count {
                        stdoutln(lines[i]);
                    }
                }
                Err(_) => {}
            }
            let _ = fs::close(fd);
        }
        Err(_) => {
            stdoutln("sort: no such file");
        }
    }
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
