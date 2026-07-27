//! join — join lines from two files on a common field
//!
//! Usage: join FILE1 FILE2
//!
//! Joins lines from two files on the first field (space/tab separated).
//! Both files must be sorted on the join field.

#![no_std]
#![no_main]

extern crate alloc;

mod common;

use common::{exit, stdout, stdoutln, stderrln};
use openos_sdk::fs;

const MAX_LINES: usize = 2048;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let args: alloc::vec::Vec<&str> = common::args().collect();

    if args.len() < 2 {
        stderrln("join: missing operand");
        stderrln("Usage: join FILE1 FILE2");
        exit(1);
    }

    let path1 = args[0];
    let path2 = args[1];

    let fd1 = match fs::open(path1) {
        Ok(fd) => fd,
        Err(_) => {
            stderrln("join: cannot open file1");
            exit(1);
        }
    };
    let fd2 = match fs::open(path2) {
        Ok(fd) => fd,
        Err(_) => {
            stderrln("join: cannot open file2");
            let _ = fs::close(fd1);
            exit(1);
        }
    };

    let mut buf1 = [0u8; 16384];
    let mut buf2 = [0u8; 16384];
    let n1 = fs::read(fd1, &mut buf1).unwrap_or(0);
    let n2 = fs::read(fd2, &mut buf2).unwrap_or(0);
    let _ = fs::close(fd1);
    let _ = fs::close(fd2);

    let data1 = &buf1[..n1];
    let data2 = &buf2[..n2];

    let mut lines1: [&str; MAX_LINES] = [""; MAX_LINES];
    let mut lines2: [&str; MAX_LINES] = [""; MAX_LINES];
    let count1 = extract_lines(data1, &mut lines1);
    let count2 = extract_lines(data2, &mut lines2);

    let mut i1 = 0usize;
    let mut i2 = 0usize;

    while i1 < count1 && i2 < count2 {
        let key1 = get_field(lines1[i1], 0);
        let key2 = get_field(lines2[i2], 0);

        match key1.cmp(key2) {
            core::cmp::Ordering::Less => {
                i1 += 1;
            }
            core::cmp::Ordering::Greater => {
                i2 += 1;
            }
            core::cmp::Ordering::Equal => {
                stdout(key1);
                let rest1 = get_rest(lines1[i1]);
                let rest2 = get_rest(lines2[i2]);
                if !rest1.is_empty() {
                    stdout(" ");
                    stdout(rest1);
                }
                if !rest2.is_empty() {
                    stdout(" ");
                    stdout(rest2);
                }
                stdoutln("");
                i1 += 1;
                i2 += 1;
            }
        }
    }

    exit(0);
}

/// Extract lines from a byte buffer into a fixed-size array.
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

/// Get the Nth field (0-indexed) from a line, using space/tab as delimiter.
fn get_field(line: &str, n: usize) -> &str {
    let bytes = line.as_bytes();
    let mut field_start = 0;
    let mut field_idx = 0;

    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b' ' || bytes[i] == b'\t' {
            if field_idx == n {
                return &line[field_start..i];
            }
            while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
                i += 1;
            }
            field_start = i;
            field_idx += 1;
        } else {
            i += 1;
        }
    }
    if field_idx == n {
        &line[field_start..]
    } else {
        ""
    }
}

/// Get everything after the first field.
fn get_rest(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i] != b' ' && bytes[i] != b'\t' {
        i += 1;
    }
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    if i < bytes.len() {
        &line[i..]
    } else {
        ""
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
