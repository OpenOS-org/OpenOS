//! touch — create a file or update its timestamp
//!
//! Usage: touch file

#![no_std]
#![no_main]

mod common;

use common::{exit, stderrln};
use openos_sdk::fs;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Demo: create /disk/newfile.txt
    let path = "/disk/newfile.txt";
    match fs::create(path) {
        Ok(_) => {
            // File created successfully
        }
        Err(_) => {
            stderrln("touch: cannot create file");
        }
    }
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
