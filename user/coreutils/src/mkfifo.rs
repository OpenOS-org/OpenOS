//! mkfifo — create a named pipe (FIFO)
//!
//! Usage: mkfifo NAME
//!
//! Named pipes are not supported on OpenOS. Prints an error and exits.

#![no_std]
#![no_main]

mod common;

use common::{exit, stderrln};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    stderrln("mkfifo: not supported (named pipes not available)");
    exit(1);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
