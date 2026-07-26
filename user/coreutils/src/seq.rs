//! seq — print a sequence of numbers
//!
//! Usage: seq [last] | [first] [last] | [first] [step] [last]

#![no_std]
#![no_main]

mod common;

use common::{exit, format_u64, stdoutln};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Default: print 1 to 10
    let mut num_buf = [0u8; 20];
    for i in 1..=10 {
        let s = format_u64(i, &mut num_buf);
        if let Ok(line) = core::str::from_utf8(s) {
            stdoutln(line);
        }
    }
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
