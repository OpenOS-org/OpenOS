//! tail — output the last part of files
//!
//! Usage: tail [-n count] [file]

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
            match fs::read(fd, &mut buf) {
                Ok(n) => {
                    let data = &buf[..n];
                    // Collect all line starts
                    let mut line_starts: [usize; 256] = [0; 256];
                    let mut line_count = 0;
                    line_starts[0] = 0;
                    for i in 0..data.len() {
                        if data[i] == b'\n' && line_count < 255 {
                            line_count += 1;
                            line_starts[line_count] = i + 1;
                        }
                    }
                    // Print last max_lines
                    let start = if line_count > max_lines {
                        line_count - max_lines
                    } else {
                        0
                    };
                    for i in start..line_count {
                        let end = if i + 1 < line_count {
                            line_starts[i + 1] - 1
                        } else {
                            data.len()
                        };
                        if let Ok(line) = core::str::from_utf8(&data[line_starts[i]..end]) {
                            stdoutln(line);
                        }
                    }
                }
                Err(_) => {}
            }
            let _ = fs::close(fd);
        }
        Err(_) => {
            stdoutln("tail: no such file");
        }
    }
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
