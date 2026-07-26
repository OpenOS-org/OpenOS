//! env — print the environment
//!
//! Usage: env

#![no_std]
#![no_main]

mod common;

use common::{exit, stdoutln};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    stdoutln("PATH=/disk:/bin");
    stdoutln("HOME=/");
    stdoutln("USER=root");
    stdoutln("SHELL=/disk/shell_rs.elf");
    stdoutln("TERM=vt100");
    stdoutln("HOSTNAME=openos");
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
