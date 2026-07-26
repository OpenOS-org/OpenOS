//! echo — display a line of text
//!
//! Usage: echo [string...]

#![no_std]
#![no_main]

mod common;

use common::{exit, stdoutln};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // In a real implementation, we'd read args from the command line.
    // For now, echo a default message.
    stdoutln("Hello from OpenOS!");
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
