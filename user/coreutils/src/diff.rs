//! diff — compare files line by line
//!
//! Usage: diff file1 file2

#![no_std]
#![no_main]

mod common;

use common::{exit, stdout, stdoutln};
use openos_sdk::fs;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let path1 = "/disk/test.txt";
    let path2 = "/disk/copy.txt";

    let fd1 = match fs::open(path1) {
        Ok(fd) => fd,
        Err(_) => {
            stdoutln("diff: cannot open first file");
            exit(1);
        }
    };
    let fd2 = match fs::open(path2) {
        Ok(fd) => fd,
        Err(_) => {
            let _ = fs::close(fd1);
            stdoutln("diff: cannot open second file");
            exit(1);
        }
    };

    let mut buf1 = [0u8; 4096];
    let mut buf2 = [0u8; 4096];
    let n1 = fs::read(fd1, &mut buf1).unwrap_or(0);
    let n2 = fs::read(fd2, &mut buf2).unwrap_or(0);

    let _ = fs::close(fd1);
    let _ = fs::close(fd2);

    if n1 == n2 && buf1[..n1] == buf2[..n2] {
        exit(0);
    }

    let data1 = &buf1[..n1];
    let data2 = &buf2[..n2];

    let mut lines1: [&str; 256] = [""; 256];
    let mut lines2: [&str; 256] = [""; 256];
    let mut count1 = 0;
    let mut count2 = 0;

    let mut start = 0;
    for i in 0..data1.len() {
        if data1[i] == b'\n' && count1 < 256 {
            if let Ok(line) = core::str::from_utf8(&data1[start..i]) {
                lines1[count1] = line;
                count1 += 1;
            }
            start = i + 1;
        }
    }
    start = 0;
    for i in 0..data2.len() {
        if data2[i] == b'\n' && count2 < 256 {
            if let Ok(line) = core::str::from_utf8(&data2[start..i]) {
                lines2[count2] = line;
                count2 += 1;
            }
            start = i + 1;
        }
    }

    let max = count1.max(count2);
    for i in 0..max {
        let l1 = if i < count1 { lines1[i] } else { "" };
        let l2 = if i < count2 { lines2[i] } else { "" };
        if l1 != l2 {
            stdout("< ");
            stdoutln(l1);
            stdout("> ");
            stdoutln(l2);
        }
    }

    exit(1);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
