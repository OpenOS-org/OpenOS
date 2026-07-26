//! ls — list directory contents
//!
//! Usage: ls [path]

#![no_std]
#![no_main]

mod common;

use common::{exit, stdoutln};
use openos_sdk::fs;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let path = "/disk";
    // Try to read the directory
    match fs::open(path) {
        Ok(fd) => {
            let mut buf = [0u8; 4096];
            match fs::read(fd, &mut buf) {
                Ok(n) => {
                    // Parse directory entries (simple format: name\0 per entry)
                    let data = &buf[..n];
                    let mut i = 0;
                    while i < data.len() {
                        let end = data[i..]
                            .iter()
                            .position(|&b| b == 0)
                            .unwrap_or(data.len() - i);
                        if end > 0 {
                            if let Ok(name) = core::str::from_utf8(&data[i..i + end]) {
                                stdoutln(name);
                            }
                        }
                        i += end + 1;
                    }
                }
                Err(_) => {
                    stdoutln("ls: read error");
                }
            }
            let _ = fs::close(fd);
        }
        Err(_) => {
            stdoutln("ls: cannot access directory");
        }
    }
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
