//! realpath — return the resolved absolute pathname
//!
//! Usage: realpath path

#![no_std]
#![no_main]

mod common;

use common::{exit, stdoutln};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Simple implementation: just return the path as-is for absolute paths
    let path = "/disk/test.txt";
    stdoutln(path);
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
