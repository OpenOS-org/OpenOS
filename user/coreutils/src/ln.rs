//! ln — create links
//!
//! Usage: ln [-s] target link_name

#![no_std]
#![no_main]

mod common;

use common::{exit, stderrln};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    stderrln("ln: not yet implemented (no link syscall)");
    exit(1);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
