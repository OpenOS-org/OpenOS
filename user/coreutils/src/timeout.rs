//! timeout — run a command with a time limit
//!
//! Usage: timeout SECONDS COMMAND [ARG...]
//!
//! Runs the given command and kills it if it does not exit within the
//! specified number of seconds. Exit status is the command's exit code,
//! or 124 if the command timed out.

#![no_std]
#![no_main]

extern crate alloc;

mod common;

use alloc::string::String;

use common::{exit, stderrln, stdoutln};
use openos_sdk::{env, process, time};

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

    // First argument: duration in seconds.
    let duration_str = match args_iter.next() {
        Some(a) => a,
        None => {
            stderrln("timeout: missing operand");
            stderrln("Usage: timeout SECONDS COMMAND [ARG...]");
            exit(1);
            unreachable!()
        }
    };

    let seconds = match parse_u64(duration_str) {
        Some(s) if s > 0 => s,
        _ => {
            stderrln("timeout: invalid duration");
            exit(1);
            unreachable!()
        }
    };

    // Second argument: command name.
    let command = match args_iter.next() {
        Some(c) => c,
        None => {
            stderrln("timeout: missing command");
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

    // Compute timeout in ticks. The timer runs at 100 Hz (10 ms per tick).
    const TICKS_PER_SEC: u64 = 100;
    let timeout_ticks = seconds.saturating_mul(TICKS_PER_SEC);

    // Create and start the child process.
    let task_id = match process::create(command) {
        Ok(id) => id,
        Err(_) => {
            stderrln("timeout: failed to create process");
            exit(1);
            unreachable!()
        }
    };

    // Set __ARGS__ so the child can read its arguments.
    let _ = env::set("__ARGS__", &child_args);

    if process::start(task_id, command).is_err() {
        stderrln("timeout: failed to start command");
        exit(1);
    }

    // Wait for the child with a timeout.
    match process::wait(task_id, timeout_ticks) {
        Ok(exit_code) => {
            // Command exited within the time limit.
            exit(exit_code as i32);
        }
        Err(_) => {
            // Timed out — kill the child.
            let _ = openos_sdk::signal::kill(task_id, openos_sdk::signal::SIGKILL);
            stdoutln("timeout: timed out");
            exit(124);
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
