//! rmdir — remove empty directories
//!
//! Usage: rmdir directory

#![no_std]
#![no_main]

mod common;

use common::{exit, stderrln};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    stderrln("rmdir: not yet implemented");
    exit(1);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
