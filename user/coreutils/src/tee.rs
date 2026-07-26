//! tee — read from stdin and write to stdout and file
//!
//! Usage: tee file

#![no_std]
#![no_main]

mod common;

use common::exit;
use openos_sdk::{console, fs};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Demo: read from console and write to file
    let path = "/disk/tee_output.txt";
    let fd = match fs::create(path) {
        Ok(fd) => fd,
        Err(_) => {
            let _ = console::writeln("tee: cannot create file");
            exit(1);
        }
    };

    let mut buf = [0u8; 256];
    loop {
        match console::read(&mut buf, true) {
            Ok(0) => break,
            Ok(n) => {
                let _ = console::write(core::str::from_utf8(&buf[..n]).unwrap_or(""));
                let _ = fs::write(fd, &buf[..n]);
            }
            Err(_) => break,
        }
    }

    let _ = fs::close(fd);
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
