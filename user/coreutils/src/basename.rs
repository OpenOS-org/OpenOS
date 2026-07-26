//! basename — strip directory and suffix from filenames
//!
//! Usage: basename path [suffix]

#![no_std]
#![no_main]

mod common;

use common::{exit, stdoutln};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Demo: extract basename from a path
    let path = "/disk/test.txt";
    let name = path.rsplit('/').next().unwrap_or(path);
    // Strip .txt suffix
    let name = name.strip_suffix(".txt").unwrap_or(name);
    stdoutln(name);
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
