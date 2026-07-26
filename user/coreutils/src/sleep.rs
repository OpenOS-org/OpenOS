//! sleep — delay for a specified time
//!
//! Usage: sleep seconds

#![no_std]
#![no_main]

mod common;

use common::exit;
use openos_sdk::console;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Default: sleep for ~1 second (18 ticks at 18.2 Hz)
    // In a real implementation, parse the argument.
    let _ = console::writeln("sleeping...");
    // Approximate 1 second at 18.2 Hz timer
    for _ in 0..18 {
        // Each tick is ~55ms, so 18 ticks ≈ 1 second
        // Use a busy wait loop as a placeholder
        for _ in 0..1000000 {
            unsafe {
                core::arch::asm!("nop");
            }
        }
    }
    let _ = console::writeln("done");
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
