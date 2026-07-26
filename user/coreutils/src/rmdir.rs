//! rmdir — remove empty directories
//!
//! Usage: rmdir [-p] directory [...]

#![no_std]
#![no_main]

mod common;

use common::{exit, stderrln};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let mut remove_parents = false;
    let mut found_dir = false;

    for arg in common::args() {
        if arg == "-p" {
            remove_parents = true;
            continue;
        }

        found_dir = true;

        if let Err(_e) = openos_sdk::fs::rmdir(arg) {
            common::stderr_fmt("rmdir: failed to remove", arg);
            exit(1);
        }

        if remove_parents {
            // Try to remove parent directories up to root.
            let mut path = arg;
            while let Some(pos) = path.rfind('/') {
                if pos == 0 {
                    break;
                }
                path = &path[..pos];
                let _ = openos_sdk::fs::rmdir(path);
            }
        }
    }

    if !found_dir {
        stderrln("rmdir: missing operand");
        exit(1);
    }

    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
