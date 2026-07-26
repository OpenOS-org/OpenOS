//! cut — remove sections from each line of files
//!
//! Usage: cut -b list [file] | -c list [file] | -f list [-d delim] [file]

#![no_std]
#![no_main]

mod common;

use common::{exit, stdoutln};
use openos_sdk::fs;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Demo: cut first 10 characters from each line
    let path = "/disk/test.txt";
    let max_chars = 10;

    match fs::open(path) {
        Ok(fd) => {
            let mut buf = [0u8; 8192];
            match fs::read(fd, &mut buf) {
                Ok(n) => {
                    let data = &buf[..n];
                    let mut start = 0;
                    for i in 0..data.len() {
                        if data[i] == b'\n' {
                            let end = (i - start).min(max_chars);
                            if let Ok(line) = core::str::from_utf8(&data[start..start + end]) {
                                stdoutln(line);
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
            stdoutln("cut: no such file");
        }
    }
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
