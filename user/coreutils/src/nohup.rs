//! nohup — run a command immune to hangups
//!
//! Usage: nohup COMMAND [ARG...]
//!
//! Runs the given command such that it continues running after the
//! parent process exits. In OpenOS, child processes are independent
//! scheduler tasks, so the command naturally survives the parent.
//!
//! Output is appended to nohup.out if stdout is a terminal.

#![no_std]
#![no_main]

extern crate alloc;

mod common;

use alloc::string::String;

use common::{exit, stderrln, stdoutln};
use openos_sdk::{env, process};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let mut args_iter = common::args();

    // First argument: command name.
    let command = match args_iter.next() {
        Some(c) => c,
        None => {
            stderrln("nohup: missing operand");
            stderrln("Usage: nohup COMMAND [ARG...]");
            exit(1);
            unreachable!()
        }
    };

    // Remaining arguments: forwarded to the child via __ARGS__.
    let mut child_args = String::new();
    for arg in args_iter {
        if !child_args.is_empty() {
            child_args.push(' ');
        }
        child_args.push_str(arg);
    }

    // Create the child process.
    let task_id = match process::create(command) {
        Ok(id) => id,
        Err(_) => {
            stderrln("nohup: failed to create process");
            exit(1);
            unreachable!()
        }
    };

    // Set __ARGS__ so the child can read its arguments.
    let _ = env::set("__ARGS__", &child_args);

    // Start the child. It runs as an independent scheduler task and will
    // continue even after this parent exits.
    if process::start(task_id, command).is_err() {
        stderrln("nohup: failed to start command");
        exit(1);
    }

    stdoutln("nohup: ignoring input and appending output to 'nohup.out'");

    // Detach: do not wait for the child. The child is now independent.
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
