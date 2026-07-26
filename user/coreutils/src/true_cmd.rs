//! true — do nothing, successfully
//!
//! Usage: true

#![no_std]
#![no_main]

mod common;

use common::exit;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
