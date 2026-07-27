//! column — columnate lists
//!
//! Usage: column [file]
//!
//! Reads lines from a file and formats them into columns filling rows first
//! (left-to-right, top-to-bottom).

#![no_std]
#![no_main]

extern crate alloc;

mod common;

use common::{exit, stderrln, stdout_byte, stdout_bytes};
use openos_sdk::fs;

const TERM_WIDTH: usize = 80;
const MAX_LINES: usize = 1024;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let args: alloc::vec::Vec<&str> = common::args().collect();

    let path = if !args.is_empty() { args[0] } else { "" };

    let mut buf = [0u8; 16384];

    let n = if !path.is_empty() {
        let fd = match fs::open(path) {
            Ok(fd) => fd,
            Err(_) => {
                stderrln("column: cannot open file");
                exit(1);
            }
        };
        let n = match fs::read(fd, &mut buf) {
            Ok(n) => n,
            Err(_) => {
                let _ = fs::close(fd);
                stderrln("column: read error");
                exit(1);
            }
        };
        let _ = fs::close(fd);
        n
    } else {
        // No file: read from default demo file.
        let fd = match fs::open("/disk/column.txt") {
            Ok(fd) => fd,
            Err(_) => {
                stderrln("column: no input");
                exit(1);
            }
        };
        let n = match fs::read(fd, &mut buf) {
            Ok(n) => n,
            Err(_) => {
                let _ = fs::close(fd);
                stderrln("column: read error");
                exit(1);
            }
        };
        let _ = fs::close(fd);
        n
    };

    let data = &buf[..n];

    let mut lines: [&str; MAX_LINES] = [""; MAX_LINES];
    let count = extract_lines(data, &mut lines);

    if count == 0 {
        exit(0);
    }

    let max_width = lines[..count].iter().map(|l| l.len()).max().unwrap_or(0);

    let col_width = (max_width + 2).min(TERM_WIDTH);
    let cols_per_row = if col_width > 0 {
        let c = TERM_WIDTH / col_width;
        if c < 1 {
            1
        } else {
            c
        }
    } else {
        1
    };
    let rows = (count + cols_per_row - 1) / cols_per_row;

    for row in 0..rows {
        for col in 0..cols_per_row {
            let idx = row + col * rows;
            if idx < count {
                let line = lines[idx];
                stdout_bytes(line.as_bytes());
                if col < cols_per_row - 1 {
                    let padding = col_width - line.len().min(col_width);
                    for _ in 0..padding {
                        stdout_byte(b' ');
                    }
                }
            }
        }
        stdout_byte(b'\n');
    }

    exit(0);
}

/// Extract non-empty lines from a byte buffer into a fixed-size array.
fn extract_lines<'a>(data: &'a [u8], lines: &mut [&'a str; MAX_LINES]) -> usize {
    let mut count = 0;
    let mut start = 0;
    for i in 0..data.len() {
        if data[i] == b'\n' {
            if count < MAX_LINES {
                if let Ok(line) = core::str::from_utf8(&data[start..i]) {
                    if !line.is_empty() {
                        lines[count] = line;
                        count += 1;
                    }
                }
            }
            start = i + 1;
        }
    }
    if start < data.len() && count < MAX_LINES {
        if let Ok(line) = core::str::from_utf8(&data[start..]) {
            if !line.is_empty() {
                lines[count] = line;
                count += 1;
            }
        }
    }
    count
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
