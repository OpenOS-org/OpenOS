//! Hello world in Rust using the OpenOS SDK.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use openos_sdk::{channel, console};

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = console::writeln("PANIC in user-space!");
    openos_sdk::process::exit(1);
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Write to kernel debug console via syscall.
    let _ = console::writeln("Hello from Rust user-space!");

    // Create a channel and send a message.
    if let Ok((handle_a, _handle_b)) = channel::create() {
        let _ = channel::send(handle_a, b"Rust SDK works!");
    }

    // Exit cleanly.
    openos_sdk::process::exit(0);
}
