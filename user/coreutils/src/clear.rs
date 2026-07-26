//! clear — clear the terminal screen
//!
//! Usage: clear

#![no_std]
#![no_main]

mod common;

use common::exit;
use openos_sdk::console;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // ANSI escape: clear screen + move cursor to top-left
    let _ = console::write("\x1b[2J\x1b[H");
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
