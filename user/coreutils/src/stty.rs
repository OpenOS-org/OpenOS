//! stty — change and print terminal line settings
//!
//! Usage: stty [-a]

#![no_std]
#![no_main]

mod common;

use common::{exit, stdoutln};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    stdoutln("speed 115200 baud; line = 0;");
    stdoutln("-brkint -imaxbel");
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
