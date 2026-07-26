//! mkdir — create directories
//!
//! Usage: mkdir [-p] directory [...]

#![no_std]
#![no_main]

mod common;

use common::{exit, stderrln};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let mut create_parents = false;
    let mut found_dir = false;

    for arg in common::args() {
        if arg == "-p" {
            create_parents = true;
            continue;
        }

        found_dir = true;

        if create_parents {
            // Create parent directories as needed by iterating path components.
            let mut path = common::String::new();
            for component in arg.split('/') {
                if component.is_empty() {
                    path.push('/');
                    continue;
                }
                if !path.is_empty() && !path.ends_with('/') {
                    path.push('/');
                }
                path.push_str(component);
                // Ignore errors for intermediate dirs (they may already exist).
                let _ = openos_sdk::fs::mkdir(&path);
            }
        } else if let Err(_e) = openos_sdk::fs::mkdir(arg) {
            common::stderr_fmt("mkdir: cannot create directory", arg);
            exit(1);
        }
    }

    if !found_dir {
        stderrln("mkdir: missing operand");
        exit(1);
    }

    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
