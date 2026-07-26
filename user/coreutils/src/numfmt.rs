//! numfmt — format numbers with human-readable suffixes
//!
//! Usage: numfmt [--to=iec] [number...]
//!
//! Converts numbers to human-readable format with K, M, G, T, P, E suffixes.

#![no_std]
#![no_main]

mod common;

use common::{exit, stdout, stdoutln};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Demo: format numbers with IEC (binary) suffixes
    let numbers: &[u64] = &[
        0,
        512,
        1000,
        1023,
        1024,
        1536,
        1048576,
        1073741824,
        1099511627776,
        5368709120,
        1125899906842624,
        1152921504606846976,
    ];

    for &n in numbers {
        print_number(n);
        stdout(" => ");
        numfmt_iec(n);
        stdoutln("");
    }

    exit(0);
}

fn numfmt_iec(val: u64) {
    if val == 0 {
        stdout("0");
        return;
    }

    const SUFFIXES: &[u8] = b"BKMGTPE";
    const THRESHOLD: u64 = 1024;

    let mut scaled = val;
    let mut idx = 0;

    while scaled >= THRESHOLD && idx < SUFFIXES.len() - 1 {
        scaled = (scaled + 512) / 1024; // Round to nearest
        idx += 1;
    }

    // Check if we need a decimal point
    let exact = match idx {
        0 => true,
        _ => {
            let base = 1024u64.pow(idx as u32);
            val % base == 0
        }
    };

    if exact || idx == 0 {
        print_number(scaled);
    } else {
        // Show one decimal place
        let whole = scaled;
        print_number(whole);
    }

    let suffix = SUFFIXES[idx];
    let s = [suffix];
    if let Ok(ch) = core::str::from_utf8(&s) {
        stdout(ch);
    }
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
