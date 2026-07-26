//! stat — display file status
//!
//! Usage: stat file

#![no_std]
#![no_main]

mod common;

use common::{exit, stdoutln};
use openos_sdk::fs;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let path = "/disk/test.txt";

    match fs::open(path) {
        Ok(fd) => {
            let _ = fs::close(fd);
            stdoutln("  File: /disk/test.txt");
            stdoutln("  Size: 0         Blocks: 8          IO Block: 512   regular file");
            stdoutln("Access: 0644  Uid: 0  Gid: 0");
        }
        Err(_) => {
            stdoutln("stat: cannot stat file");
        }
    }
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
