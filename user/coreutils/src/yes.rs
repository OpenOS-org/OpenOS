//! yes — output a string repeatedly until killed
//!
//! Usage: yes [string]

#![no_std]
#![no_main]

mod common;

use common::{exit, stdoutln};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    loop {
        stdoutln("y");
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
