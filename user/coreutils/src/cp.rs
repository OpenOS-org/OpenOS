//! cp — copy files
//!
//! Usage: cp source dest

#![no_std]
#![no_main]

mod common;

use common::{exit, stderrln};
use openos_sdk::fs;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Demo: copy /disk/test.txt to /disk/copy.txt
    let src = "/disk/test.txt";
    let dst = "/disk/copy.txt";

    let src_fd = match fs::open(src) {
        Ok(fd) => fd,
        Err(_) => {
            stderrln("cp: cannot open source");
            exit(1);
        }
    };

    let dst_fd = match fs::create(dst) {
        Ok(fd) => fd,
        Err(_) => {
            stderrln("cp: cannot create destination");
            let _ = fs::close(src_fd);
            exit(1);
        }
    };

    let mut buf = [0u8; 4096];
    loop {
        match fs::read(src_fd, &mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let _ = fs::write(dst_fd, &buf[..n]);
            }
            Err(_) => break,
        }
    }

    let _ = fs::close(src_fd);
    let _ = fs::close(dst_fd);
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
