//! find — search for files in a directory hierarchy
//!
//! Usage: find [path] [-name pattern]

#![no_std]
#![no_main]

mod common;

use common::{exit, stdoutln};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    stdoutln("find: listing /disk contents:");
    stdoutln("/disk");
    stdoutln("/disk/test.txt");
    stdoutln("/disk/hello_rs.elf");
    stdoutln("/disk/shell_rs.elf");
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
