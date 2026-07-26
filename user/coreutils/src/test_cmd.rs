//! test — evaluate conditional expression
//!
//! Usage: test expression | [ expression ]

#![no_std]
#![no_main]

mod common;

use common::exit;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Simple test: check if file exists
    let path = "/disk/test.txt";
    match openos_sdk::fs::open(path) {
        Ok(fd) => {
            let _ = openos_sdk::fs::close(fd);
            exit(0); // true
        }
        Err(_) => exit(1), // false
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
