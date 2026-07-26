//! install — copy files and set permissions
//!
//! Usage: install [-m MODE] SOURCE DEST

#![no_std]
#![no_main]

mod common;

use common::{copy_fd, exit, stderrln, stdoutln};
use openos_sdk::fs;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Parse arguments: install [-m MODE] SOURCE DEST
    let mut args_iter = common::args();
    let mut mode: Option<&str> = None;
    let mut source: Option<&str> = None;
    let mut dest: Option<&str> = None;

    while let Some(arg) = args_iter.next() {
        if arg == "-m" {
            mode = args_iter.next();
        } else if source.is_none() {
            source = Some(arg);
        } else if dest.is_none() {
            dest = Some(arg);
        }
    }

    let source = match source {
        Some(s) => s,
        None => {
            stderrln("install: missing operand");
            exit(1);
        }
    };

    let dest = match dest {
        Some(d) => d,
        None => {
            stderrln("install: missing destination operand");
            exit(1);
        }
    };

    // Open source file.
    let src_fd = match fs::open(source) {
        Ok(fd) => fd,
        Err(_) => {
            stderrln("install: cannot open source");
            exit(1);
        }
    };

    // Create destination file.
    let dst_fd = match fs::create(dest) {
        Ok(fd) => fd,
        Err(_) => {
            stderrln("install: cannot create destination");
            let _ = fs::close(src_fd);
            exit(1);
        }
    };

    // Copy contents.
    match copy_fd(src_fd, dst_fd) {
        Ok(bytes) => {
            stdoutln("install: copied file");
            let _ = bytes; // suppress unused warning
        }
        Err(_) => {
            stderrln("install: copy failed");
            let _ = fs::close(src_fd);
            let _ = fs::close(dst_fd);
            exit(1);
        }
    }

    let _ = fs::close(src_fd);
    let _ = fs::close(dst_fd);

    // Log the mode if provided (chmod not yet implemented).
    if let Some(m) = mode {
        let _ = m; // chmod syscall not available yet
    }

    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
