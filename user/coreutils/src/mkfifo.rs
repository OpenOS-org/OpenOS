//! mkfifo — create a named pipe (FIFO)
//!
//! Creates a FIFO special file at each given path. Once created, the FIFO
//! can be opened and used like an anonymous pipe for inter-process
//! communication.
//!
//! Usage: mkfifo [OPTION]... NAME...
//!
//! Options:
//!   --help     display this help and exit
//!
//! Examples:
//!   mkfifo mypipe        create a FIFO named "mypipe"

#![no_std]
#![no_main]

mod common;

use common::{exit, print, stderrln};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let args = common::args();
    let mut exit_code: i32 = 0;

    if args.len() < 2 {
        stderrln("mkfifo: missing operand");
        stderrln("usage: mkfifo NAME...");
        exit(1);
    }

    for arg in &args[1..] {
        if *arg == "--help" {
            print("usage: mkfifo NAME...\n");
            exit(0);
        }

        match openos_sdk::fs::mkfifo(arg) {
            Ok(()) => {
                // Success.
            }
            Err(e) => {
                stderrln(&alloc::format!(
                    "mkfifo: cannot create fifo '{}': {:?}",
                    arg,
                    e
                ));
                exit_code = 1;
            }
        }
    }

    exit(exit_code);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
