//! dirname — strip last component from filename
//!
//! Usage: dirname path

#![no_std]
#![no_main]

mod common;

use common::{exit, stdoutln};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let path = "/disk/test.txt";
    match path.rfind('/') {
        Some(pos) => {
            if pos == 0 {
                stdoutln("/");
            } else {
                stdoutln(&path[..pos]);
            }
        }
        None => stdoutln("."),
    }
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
