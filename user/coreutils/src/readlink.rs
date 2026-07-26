//! readlink — print value of a symbolic link
//!
//! Usage: readlink link

#![no_std]
#![no_main]

mod common;

use common::{exit, stderrln};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    stderrln("readlink: not implemented (no symlink support)");
    exit(1);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
