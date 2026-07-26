//! nproc — print the number of processing units available
//!
//! Usage: nproc [--all]
//!
//! Returns 1 as default (single-core assumption until SMP is queried).

#![no_std]
#![no_main]

mod common;

use common::{exit, stdoutln};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Return 1 as the default number of processors.
    // A full implementation would query the kernel's SMP info.
    stdoutln("1");
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
