//! shuf — shuffle lines of input
//!
//! Usage: shuf [file]
//!
//! Uses a simple LCG PRNG for shuffling (Fisher-Yates on a fixed buffer).

#![no_std]
#![no_main]

mod common;

use common::{exit, stderrln, stdoutln};
use openos_sdk::fs;

struct LcgRng {
    state: u64,
}

impl LcgRng {
    fn new(seed: u64) -> Self {
        Self {
            state: seed.wrapping_add(1),
        }
    }

    fn next_u32(&mut self) -> u32 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.state >> 33) ^ self.state) as u32
    }

    fn range(&mut self, max: u32) -> u32 {
        self.next_u32() % max
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let path = "/disk/test.txt";

    match fs::open(path) {
        Ok(fd) => {
            let mut buf = [0u8; 8192];
            match fs::read(fd, &mut buf) {
                Ok(n) => {
                    let data = &buf[..n];
                    // Extract lines
                    let mut lines: [&str; 256] = [""; 256];
                    let mut count = 0;
                    let mut start = 0;
                    for i in 0..data.len() {
                        if data[i] == b'\n' {
                            if count < 256 {
                                if let Ok(line) = core::str::from_utf8(&data[start..i]) {
                                    lines[count] = line;
                                    count += 1;
                                }
                            }
                            start = i + 1;
                        }
                    }
                    // Last line without trailing newline
                    if start < data.len() && count < 256 {
                        if let Ok(line) = core::str::from_utf8(&data[start..]) {
                            lines[count] = line;
                            count += 1;
                        }
                    }

                    // Seed from a simple counter
                    static mut COUNTER: u64 = 0;
                    let seed = unsafe {
                        COUNTER = COUNTER.wrapping_add(1);
                        COUNTER
                    };
                    let mut rng = LcgRng::new(seed.wrapping_add(count as u64));

                    // Fisher-Yates shuffle
                    for i in (1..count).rev() {
                        let j = rng.range((i + 1) as u32) as usize;
                        lines.swap(i, j);
                    }

                    // Print shuffled lines
                    for i in 0..count {
                        stdoutln(lines[i]);
                    }
                }
                Err(_) => {
                    stderrln("shuf: read error");
                }
            }
            let _ = fs::close(fd);
        }
        Err(_) => {
            stderrln("shuf: no such file");
        }
    }
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
