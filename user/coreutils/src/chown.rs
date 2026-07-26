//! chown — change file owner and group (stub)
//!
//! Usage: chown [OWNER][:[GROUP]] FILE...
//!
//! This is a stub implementation. The chown syscall does not exist yet.
//! It parses arguments and prints what it would do.

#![no_std]
#![no_main]

mod common;

use common::{exit, stderrln, stdoutln};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Demo: print what chown would do
    // In a real shell, OWNER:GROUP and FILE come from argv.
    let owner_spec = "user";
    let group_spec = "staff";
    let path = "/disk/test.txt";

    // Validate the owner:group spec
    let has_owner = !owner_spec.is_empty();
    let has_group = !group_spec.is_empty();

    if !has_owner && !has_group {
        stderrln("chown: missing operand");
        exit(1);
    }

    if has_owner && has_group {
        let _ = openos_sdk::console::write("chown: would change owner of '");
        let _ = openos_sdk::console::write(path);
        let _ = openos_sdk::console::write("' to ");
        let _ = openos_sdk::console::write(owner_spec);
        let _ = openos_sdk::console::write(":");
        let _ = openos_sdk::console::write(group_spec);
        stdoutln("");
    } else if has_owner {
        let _ = openos_sdk::console::write("chown: would change owner of '");
        let _ = openos_sdk::console::write(path);
        let _ = openos_sdk::console::write("' to ");
        let _ = openos_sdk::console::write(owner_spec);
        stdoutln("");
    } else {
        let _ = openos_sdk::console::write("chown: would change group of '");
        let _ = openos_sdk::console::write(path);
        let _ = openos_sdk::console::write("' to ");
        let _ = openos_sdk::console::write(group_spec);
        stdoutln("");
    }

    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
