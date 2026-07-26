//! nl — number lines of files
//!
//! Usage: nl [-b a|t] [file]
//!
//! -b a  number all lines (default)
//! -b t  number only non-empty lines

#![no_std]
#![no_main]

mod common;

use common::{exit, format_u64, stderrln, stdout, stdoutln};
use openos_sdk::fs;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let path = "/disk/test.txt";
    let number_all = true; // default: -b a

    match fs::open(path) {
        Ok(fd) => {
            let mut buf = [0u8; 8192];
            match fs::read(fd, &mut buf) {
                Ok(n) => {
                    let data = &buf[..n];
                    let mut line_num: u64 = 1;
                    let mut start = 0;

                    for i in 0..data.len() {
                        if data[i] == b'\n' {
                            let line = &data[start..i];
                            let is_empty = line.iter().all(|b| b.is_ascii_whitespace());

                            if number_all || !is_empty {
                                print_numbered_line(line_num, line);
                                line_num += 1;
                            } else {
                                // Print blank padding for un-numbered lines
                                stdout("      \t");
                                if let Ok(s) = core::str::from_utf8(line) {
                                    stdoutln(s);
                                } else {
                                    stdoutln("");
                                }
                            }
                            start = i + 1;
                        }
                    }
                    // Last line without trailing newline
                    if start < data.len() {
                        let line = &data[start..];
                        let is_empty = line.iter().all(|b| b.is_ascii_whitespace());

                        if number_all || !is_empty {
                            print_numbered_line(line_num, line);
                        } else if let Ok(s) = core::str::from_utf8(line) {
                            stdout("      \t");
                            stdoutln(s);
                        }
                    }
                }
                Err(_) => {
                    stderrln("nl: read error");
                }
            }
            let _ = fs::close(fd);
        }
        Err(_) => {
            stderrln("nl: no such file");
        }
    }
    exit(0);
}

fn print_numbered_line(num: u64, line: &[u8]) {
    // Right-justify number in 6 columns
    let mut num_buf = [0u8; 20];
    let s = format_u64(num, &mut num_buf);
    let num_str = core::str::from_utf8(s).unwrap_or("?");
    let padding = if num_str.len() < 6 {
        6 - num_str.len()
    } else {
        0
    };
    for _ in 0..padding {
        stdout(" ");
    }
    stdout(num_str);
    stdout("\t");
    if let Ok(text) = core::str::from_utf8(line) {
        stdoutln(text);
    } else {
        stdoutln("");
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
