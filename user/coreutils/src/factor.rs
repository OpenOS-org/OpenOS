//! factor — factor integers into prime factors
//!
//! Usage: factor [number...]
//!
//! Reads numbers from arguments or prints a demo factorization.

#![no_std]
#![no_main]

mod common;

use common::{exit, stdout, stdoutln};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Demo: factor some numbers
    let numbers: &[u64] = &[12, 35, 100, 1024, 1234567, 97, 2, 60];

    for &n in numbers {
        stdout("factor: ");
        print_number(n);
        stdout(": ");
        factor_number(n);
    }

    exit(0);
}

fn factor_number(mut n: u64) {
    if n <= 1 {
        stdoutln("");
        return;
    }

    let mut first = true;
    // Handle factor 2 separately for efficiency
    while n % 2 == 0 {
        if !first {
            stdout(" ");
        }
        stdout("2");
        n /= 2;
        first = false;
    }

    // Try odd factors starting from 3
    let mut d = 3u64;
    while d * d <= n {
        while n % d == 0 {
            if !first {
                stdout(" ");
            }
            print_number(d);
            n /= d;
            first = false;
        }
        d += 2;
    }

    // If n is still greater than 1, it's a prime factor
    if n > 1 {
        if !first {
            stdout(" ");
        }
        print_number(n);
    }
    stdoutln("");
}

fn print_number(mut val: u64) {
    if val == 0 {
        stdout("0");
        return;
    }
    let mut buf = [0u8; 20];
    let mut pos = 20;
    while val > 0 {
        pos -= 1;
        buf[pos] = b'0' + (val % 10) as u8;
        val /= 10;
    }
    if let Ok(s) = core::str::from_utf8(&buf[pos..]) {
        stdout(s);
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
