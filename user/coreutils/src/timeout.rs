//! timeout — run a command with a time limit (stub)
//!
//! Usage: timeout DURATION COMMAND [ARG...]
//!
//! This is a stub. The kernel does not yet support process creation
//! from user-space with argument passing. Prints a diagnostic message.

#![no_std]
#![no_main]

mod common;

use common::{exit, stderrln, stdoutln};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // In a real shell, DURATION and COMMAND come from argv.
    let duration = "10";
    let command = "ls";

    let _ = openos_sdk::console::write("timeout: would run '");
    let _ = openos_sdk::console::write(command);
    let _ = openos_sdk::console::write("' with ");
    let _ = openos_sdk::console::write(duration);
    stdoutln("s limit");
    stderrln("timeout: not yet supported");
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
