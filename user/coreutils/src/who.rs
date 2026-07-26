//! who — show who is logged in
//!
//! Usage: who
//!
//! Stub implementation — prints the current user.

#![no_std]
#![no_main]

mod common;

use common::{exit, stdoutln};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    stdoutln("root     tty1         2026-07-27 00:00");
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
