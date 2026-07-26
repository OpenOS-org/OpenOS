//! od — dump files in octal and other formats
//!
//! Usage: od [file]

#![no_std]
#![no_main]

mod common;

use common::{exit, stdout, stdoutln};
use openos_sdk::fs;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let path = "/disk/test.txt";

    match fs::open(path) {
        Ok(fd) => {
            let mut buf = [0u8; 256];
            match fs::read(fd, &mut buf) {
                Ok(n) => {
                    let data = &buf[..n];
                    let mut offset = 0;
                    while offset < data.len() {
                        // Print offset in octal
                        let mut oct_buf = [0u8; 12];
                        let mut tmp = offset;
                        let mut pos = 11;
                        if tmp == 0 {
                            oct_buf[pos] = b'0';
                            pos -= 1;
                        }
                        while tmp > 0 {
                            oct_buf[pos] = b'0' + (tmp % 8) as u8;
                            tmp /= 8;
                            if pos > 0 {
                                pos -= 1;
                            }
                        }
                        let _ = core::str::from_utf8(&oct_buf[pos + 1..12]).map(|s| stdout(s));
                        stdout(" ");

                        // Print bytes in octal
                        let end = (offset + 16).min(data.len());
                        for i in offset..end {
                            let b = data[i];
                            let hi = (b >> 6) & 3;
                            let mid = (b >> 3) & 7;
                            let lo = b & 7;
                            let mut oct = [0u8; 4];
                            oct[0] = b'0' + hi;
                            oct[1] = b'0' + mid;
                            oct[2] = b'0' + lo;
                            oct[3] = b' ';
                            let _ = core::str::from_utf8(&oct).map(|s| stdout(s));
                        }
                        stdoutln("");

                        offset = end;
                    }
                }
                Err(_) => {}
            }
            let _ = fs::close(fd);
        }
        Err(_) => {
            stdoutln("od: no such file");
        }
    }
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
