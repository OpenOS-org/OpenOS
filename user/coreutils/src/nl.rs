//! nl — number lines of files
//!
//! Usage: nl [-b a|t] [-w width] [file]
//!
//! -b a  number all lines (default)
//! -b t  number only non-empty lines
//! -w N  use N columns for line numbers (default: 6)

#![no_std]
#![no_main]

extern crate alloc;

mod common;

use common::{args, exit, format_u64, stderrln, stdout, stdoutln};
use openos_sdk::fs;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let mut number_all = true;
    let mut width: usize = 6;
    let mut path: Option<&str> = None;

    let mut arg_iter = args();
    while let Some(arg) = arg_iter.next() {
        match arg {
            "-b" => {
                if let Some(mode) = arg_iter.next() {
                    number_all = mode != "t";
                }
            }
            "-w" => {
                if let Some(w) = arg_iter.next() {
                    width = parse_usize(w).unwrap_or(6);
                }
            }
            _ => {
                path = Some(arg);
            }
        }
    }

    let Some(file_path) = path else {
        stderrln("nl: missing file operand");
        exit(1);
    };

    match fs::open(file_path) {
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
                                print_numbered_line(line_num, line, width);
                                line_num += 1;
                            } else {
                                // Print blank padding for un-numbered lines.
                                for _ in 0..width {
                                    stdout(" ");
                                }
                                stdout("\t");
                                if let Ok(s) = core::str::from_utf8(line) {
                                    stdoutln(s);
                                } else {
                                    stdoutln("");
                                }
                            }
                            start = i + 1;
                        }
                    }
                    // Last line without trailing newline.
                    if start < data.len() {
                        let line = &data[start..];
                        let is_empty = line.iter().all(|b| b.is_ascii_whitespace());

                        if number_all || !is_empty {
                            print_numbered_line(line_num, line, width);
                        } else if let Ok(s) = core::str::from_utf8(line) {
                            for _ in 0..width {
                                stdout(" ");
                            }
                            stdout("\t");
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

fn print_numbered_line(num: u64, line: &[u8], width: usize) {
    // Right-justify number in `width` columns.
    let mut num_buf = [0u8; 20];
    let s = format_u64(num, &mut num_buf);
    let num_str = core::str::from_utf8(s).unwrap_or("?");
    let padding = if num_str.len() < width {
        width - num_str.len()
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

fn parse_usize(s: &str) -> Option<usize> {
    let mut result = 0usize;
    for &b in s.as_bytes() {
        if !b.is_ascii_digit() {
            return None;
        }
        result = result.checked_mul(10)?.checked_add((b - b'0') as usize)?;
    }
    Some(result)
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
