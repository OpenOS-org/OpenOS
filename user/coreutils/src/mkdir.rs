//! mkdir — create directories
//!
//! Usage: mkdir directory

#![no_std]
#![no_main]

mod common;

use common::{exit, stderrln};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    stderrln("mkdir: not yet implemented (no mkdir syscall)");
    exit(1);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
