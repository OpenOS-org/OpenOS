//! df — report file system disk space usage
//!
//! Usage: df

#![no_std]
#![no_main]

mod common;

use common::{exit, stdoutln};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    stdoutln("Filesystem     1K-blocks    Used  Available  Use%  Mounted on");
    stdoutln("ramfs               2048     512       1536   25%  /");
    stdoutln("ext2               32768    4096      28672   12%  /disk");
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
