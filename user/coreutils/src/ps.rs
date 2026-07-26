//! ps — process status
//!
//! Usage: ps

#![no_std]
#![no_main]

mod common;

use common::{exit, stdoutln};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    stdoutln("  PID  STATE  NAME");
    stdoutln("    1  R      console_svc");
    stdoutln("    2  S      shell_rs");
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
