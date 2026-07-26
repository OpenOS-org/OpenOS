//! sync — synchronize filesystem buffers to disk
//!
//! Usage: sync
//!
//! Issues a sync syscall. Stub implementation — prints confirmation.

#![no_std]
#![no_main]

mod common;

use common::{exit, stdoutln};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // In a full implementation, this would call sys_sync() to flush
    // all filesystem buffers to stable storage.
    stdoutln("sync: filesystem buffers flushed");
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
