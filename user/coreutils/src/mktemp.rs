//! mktemp — create a temporary file or directory with a unique name
//!
//! Usage: mktemp [-d] [TEMPLATE]
//!
//! TEMPLATE must end in "XXXXXX" which is replaced with a unique suffix.

#![no_std]
#![no_main]

mod common;

use common::{exit, stderrln, stdoutln};
use openos_sdk::fs;

/// Simple LCG PRNG seeded from a monotonic counter.
struct LcgRng {
    state: u64,
}

impl LcgRng {
    fn new(seed: u64) -> Self {
        Self {
            state: seed.wrapping_add(1),
        }
    }

    fn next(&mut self) -> u64 {
        // Numerical Recipes LCG
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.state
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Demo: create a temp file in /tmp/
    // In a real shell, -d and TEMPLATE come from argv.
    let create_dir = false;
    let template_prefix = "/tmp/tmp.";

    // Use a simple counter-based seed
    static mut COUNTER: u64 = 0;
    let seed = unsafe {
        COUNTER = COUNTER.wrapping_add(1);
        COUNTER
    };

    let mut rng = LcgRng::new(seed);
    let mut name_buf = [0u8; 128];
    let mut name_len = 0;

    // Copy prefix
    for &b in template_prefix.as_bytes() {
        if name_len < name_buf.len() {
            name_buf[name_len] = b;
            name_len += 1;
        }
    }

    // Generate 6 random alphanumeric characters (replacing XXXXXX)
    let chars = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    for _ in 0..6 {
        if name_len < name_buf.len() {
            let r = rng.next() % chars.len() as u64;
            name_buf[name_len] = chars[r as usize];
            name_len += 1;
        }
    }

    let name = match core::str::from_utf8(&name_buf[..name_len]) {
        Ok(s) => s,
        Err(_) => {
            stderrln("mktemp: internal error");
            exit(1);
        }
    };

    if create_dir {
        match fs::mkdir(name) {
            Ok(()) => stdoutln(name),
            Err(_) => {
                stderrln("mktemp: failed to create directory");
                exit(1);
            }
        }
    } else {
        match fs::create(name) {
            Ok(fd) => {
                let _ = fs::close(fd);
                stdoutln(name);
            }
            Err(_) => {
                stderrln("mktemp: failed to create file");
                exit(1);
            }
        }
    }
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
