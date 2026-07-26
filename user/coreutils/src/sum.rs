//! sum — compute checksum and block count (BSD sum algorithm)
//!
//! Usage: sum [file...]
//!
//! Implements the BSD checksum algorithm (16-bit rotating checksum).

#![no_std]
#![no_main]

mod common;

use common::{exit, stderrln, stdout, stdoutln};
use openos_sdk::fs;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let path = "/disk/test.txt";

    match fs::open(path) {
        Ok(fd) => {
            let mut checksum: u32 = 0;
            let mut blocks: u32 = 0;
            let mut buf = [0u8; 4096];

            loop {
                match fs::read(fd, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        blocks += ((n + 511) / 512) as u32;
                        for &byte in &buf[..n] {
                            checksum = ((checksum >> 1) + ((checksum & 1) << 15))
                                .wrapping_add(byte as u32);
                            checksum &= 0xFFFF;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = fs::close(fd);

            // Print checksum and block count
            print_u32(checksum);
            stdout(" ");
            print_u32(blocks);
            stdout(" ");
            stdoutln(path);
        }
        Err(_) => {
            stderrln("sum: no such file");
        }
    }
    exit(0);
}

fn print_u32(mut val: u32) {
    if val == 0 {
        stdout("0");
        return;
    }
    let mut buf = [0u8; 10];
    let mut pos = 10;
    while val > 0 {
        pos -= 1;
        buf[pos] = b'0' + (val % 10) as u8;
        val /= 10;
    }
    if let Ok(s) = core::str::from_utf8(&buf[pos..]) {
        stdout(s);
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
