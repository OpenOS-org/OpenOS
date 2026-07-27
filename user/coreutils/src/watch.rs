//! watch — run a command periodically, showing output
//!
//! Usage: watch SECONDS COMMAND [ARG...]
//!
//! Repeatedly executes the given command at the specified interval,
//! clearing the screen and displaying the latest output each time.
//! Runs until killed (Ctrl-C).

#![no_std]
#![no_main]

extern crate alloc;

mod common;

use alloc::string::String;

use common::{exit, stderrln, stdoutln};
use openos_sdk::{console, env, process, time};

/// Parse a decimal string into a u64. Returns None on overflow or invalid input.
fn parse_u64(s: &str) -> Option<u64> {
    let mut val: u64 = 0;
    for &b in s.as_bytes() {
        if !b.is_ascii_digit() {
            return None;
        }
        val = val.checked_mul(10)?.checked_add((b - b'0') as u64)?;
    }
    Some(val)
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let mut args_iter = common::args();

    // First argument: interval in seconds.
    let interval_str = match args_iter.next() {
        Some(a) => a,
        None => {
            stderrln("watch: missing operand");
            stderrln("Usage: watch SECONDS COMMAND [ARG...]");
            exit(1);
            unreachable!()
        }
    };

    let seconds = match parse_u64(interval_str) {
        Some(s) if s > 0 => s,
        _ => {
            stderrln("watch: invalid interval");
            exit(1);
            unreachable!()
        }
    };

    // Second argument: command name.
    let command = match args_iter.next() {
        Some(c) => c,
        None => {
            stderrln("watch: missing command");
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

    // Compute interval in ticks (100 Hz timer).
    const TICKS_PER_SEC: u64 = 100;
    let interval_ticks = seconds.saturating_mul(TICKS_PER_SEC);

    let mut iteration: u64 = 0;

    loop {
        iteration = iteration.wrapping_add(1);

        // Clear screen using ANSI escape sequence.
        let _ = console::write("\x1b[2J\x1b[H");

        // Print header.
        let mut num_buf = [0u8; 20];
        let num_str = common::format_u64(iteration, &mut num_buf);
        if let Ok(s) = core::str::from_utf8(num_str) {
            let _ = console::write("Every ");
            let _ = console::write(interval_str);
            let _ = console::write("s: ");
            let _ = console::write(command);
            let _ = console::write("  (iteration ");
            let _ = console::write(s);
            let _ = console::writeln(")");
        }
        let _ = console::writeln("---");

        // Spawn the command as a child process.
        match process::create(command) {
            Ok(task_id) => {
                let _ = env::set("__ARGS__", &child_args);
                if process::start(task_id, command).is_err() {
                    let _ = console::writeln("watch: failed to start command");
                } else {
                    // Wait for the child to finish (up to interval).
                    let wait_ticks = if interval_ticks > 100 {
                        interval_ticks - 100
                    } else {
                        interval_ticks
                    };
                    let _ = process::wait(task_id, wait_ticks);
                }
            }
            Err(_) => {
                let _ = console::writeln("watch: failed to create process");
            }
        }

        // Sleep until the next interval.
        time::sleep(interval_ticks);
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
