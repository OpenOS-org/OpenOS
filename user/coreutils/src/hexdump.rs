//! hexdump — display file contents in hexadecimal
//!
//! Usage: hexdump [file]

#![no_std]
#![no_main]

mod common;

use common::{exit, format_hex, stdout, stdoutln};
use openos_sdk::fs;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let path = "/disk/test.txt";

    match fs::open(path) {
        Ok(fd) => {
            let mut buf = [0u8; 512];
            match fs::read(fd, &mut buf) {
                Ok(n) => {
                    let data = &buf[..n];
                    let mut offset = 0;
                    while offset < data.len() {
                        let mut hex_buf = [0u8; 18];
                        let hex = format_hex(offset as u64, &mut hex_buf);
                        let _ = core::str::from_utf8(hex).map(|s| stdout(s));
                        stdout(": ");

                        // Hex bytes
                        let end = (offset + 16).min(data.len());
                        for i in offset..end {
                            let mut byte_buf = [0u8; 4];
                            byte_buf[0] = b'0';
                            byte_buf[1] = b'x';
                            let hi = (data[i] >> 4) & 0xF;
                            let lo = data[i] & 0xF;
                            byte_buf[2] = if hi < 10 { b'0' + hi } else { b'a' + hi - 10 };
                            byte_buf[3] = if lo < 10 { b'0' + lo } else { b'a' + lo - 10 };
                            let _ = core::str::from_utf8(&byte_buf).map(|s| {
                                let _ = openos_sdk::console::write(s);
                                let _ = openos_sdk::console::write(" ");
                            });
                        }

                        // ASCII representation
                        stdout(" |");
                        for i in offset..end {
                            let b = data[i];
                            if b >= 0x20 && b < 0x7F {
                                let s = [b];
                                let _ = core::str::from_utf8(&s).map(|c| {
                                    let _ = openos_sdk::console::write(c);
                                });
                            } else {
                                let _ = openos_sdk::console::write(".");
                            }
                        }
                        stdoutln("|");

                        offset = end;
                    }
                }
                Err(_) => {}
            }
            let _ = fs::close(fd);
        }
        Err(_) => {
            stdoutln("hexdump: no such file");
        }
    }
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
