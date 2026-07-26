//! tty — print the file name of the terminal connected to standard input
//!
//! Usage: tty

#![no_std]
#![no_main]

mod common;

use common::{exit, stdoutln};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    stdoutln("/dev/console");
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
