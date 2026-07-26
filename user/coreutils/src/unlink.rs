//! unlink — remove a file
//!
//! Usage: unlink FILE

#![no_std]
#![no_main]

mod common;

use common::{exit, stderrln};
use openos_sdk::fs;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let mut args_iter = common::args();
    let path = match args_iter.next() {
        Some(p) => p,
        None => {
            stderrln("unlink: missing operand");
            exit(1);
        }
    };

    match fs::unlink(path) {
        Ok(()) => {
            exit(0);
        }
        Err(_) => {
            stderrln("unlink: cannot unlink file");
            exit(1);
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
