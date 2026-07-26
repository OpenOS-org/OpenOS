//! tac — concatenate and print files in reverse line order
//!
//! Usage: tac [file...]

#![no_std]
#![no_main]

mod common;

use common::{exit, stderrln, stdoutln};
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
                    // Collect line start/end offsets
                    let mut starts: [usize; 256] = [0; 256];
                    let mut ends: [usize; 256] = [0; 256];
                    let mut count = 0;
                    let mut line_start = 0;

                    for i in 0..data.len() {
                        if data[i] == b'\n' {
                            if count < 256 {
                                starts[count] = line_start;
                                ends[count] = i;
                                count += 1;
                            }
                            line_start = i + 1;
                        }
                    }
                    // Handle last line without trailing newline
                    if line_start < data.len() && count < 256 {
                        starts[count] = line_start;
                        ends[count] = data.len();
                        count += 1;
                    }

                    // Print lines in reverse order
                    let mut i = count;
                    while i > 0 {
                        i -= 1;
                        if let Ok(line) = core::str::from_utf8(&data[starts[i]..ends[i]]) {
                            stdoutln(line);
                        }
                    }
                }
                Err(_) => {
                    stderrln("tac: read error");
                }
            }
            let _ = fs::close(fd);
        }
        Err(_) => {
            stderrln("tac: no such file");
        }
    }
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
