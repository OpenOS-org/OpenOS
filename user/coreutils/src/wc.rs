//! wc — word, line, and byte count
//!
//! Usage: wc [file]

#![no_std]
#![no_main]

mod common;

use common::{exit, format_u64, stdoutln};
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
                    let (lines, words, bytes) = common::count_lwb(data);
                    let mut num_buf = [0u8; 20];
                    let mut out = [0u8; 128];
                    let mut pos = 0;

                    // lines
                    let s = format_u64(lines as u64, &mut num_buf);
                    out[pos..pos + s.len()].copy_from_slice(s);
                    pos += s.len();
                    out[pos] = b' ';
                    pos += 1;

                    // words
                    let s = format_u64(words as u64, &mut num_buf);
                    out[pos..pos + s.len()].copy_from_slice(s);
                    pos += s.len();
                    out[pos] = b' ';
                    pos += 1;

                    // bytes
                    let s = format_u64(bytes as u64, &mut num_buf);
                    out[pos..pos + s.len()].copy_from_slice(s);
                    pos += s.len();

                    if let Ok(result) = core::str::from_utf8(&out[..pos]) {
                        stdoutln(result);
                    }
                }
                Err(_) => {
                    stdoutln("wc: read error");
                }
            }
            let _ = fs::close(fd);
        }
        Err(_) => {
            stdoutln("wc: no such file");
        }
    }
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
