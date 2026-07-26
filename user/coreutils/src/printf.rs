//! printf — format and print arguments
//!
//! Usage: printf FORMAT [ARG...]
//!
//! Supports: %s %d %x %o %c %% \n \t \\

#![no_std]
#![no_main]

mod common;

use common::{exit, stdout};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Demo: print a formatted string
    // In a real shell, FORMAT and ARGs come from argv.
    let fmt = b"value=%d hex=0x%x str=%s char=%c nl\n";
    let args: &[&[u8]] = &[b"42", b"255", b"hello", b"A"];
    printf_format(fmt, args);
    exit(0);
}

/// Parse and execute a printf format string with the given arguments.
fn printf_format(fmt: &[u8], args: &[&[u8]]) {
    let mut arg_idx = 0;
    let mut i = 0;
    while i < fmt.len() {
        if fmt[i] == b'\\' && i + 1 < fmt.len() {
            match fmt[i + 1] {
                b'n' => stdout("\n"),
                b't' => stdout("\t"),
                b'\\' => stdout("\\"),
                b'0' => stdout("\0"),
                _ => {
                    // Unknown escape: print literally
                    let s = core::str::from_utf8(&fmt[i..i + 2]).unwrap_or("?");
                    stdout(s);
                }
            }
            i += 2;
        } else if fmt[i] == b'%' && i + 1 < fmt.len() {
            match fmt[i + 1] {
                b'%' => {
                    stdout("%");
                    i += 2;
                }
                b's' => {
                    if arg_idx < args.len() {
                        if let Ok(s) = core::str::from_utf8(args[arg_idx]) {
                            stdout(s);
                        }
                        arg_idx += 1;
                    }
                    i += 2;
                }
                b'c' => {
                    if arg_idx < args.len() && !args[arg_idx].is_empty() {
                        let c = [args[arg_idx][0]];
                        if let Ok(s) = core::str::from_utf8(&c) {
                            stdout(s);
                        }
                        arg_idx += 1;
                    }
                    i += 2;
                }
                b'd' => {
                    if arg_idx < args.len() {
                        if let Ok(s) = core::str::from_utf8(args[arg_idx]) {
                            if let Ok(val) = parse_i64(s) {
                                print_i64(val);
                            }
                        }
                        arg_idx += 1;
                    }
                    i += 2;
                }
                b'x' => {
                    if arg_idx < args.len() {
                        if let Ok(s) = core::str::from_utf8(args[arg_idx]) {
                            if let Ok(val) = parse_u64_hex(s) {
                                print_hex(val);
                            }
                        }
                        arg_idx += 1;
                    }
                    i += 2;
                }
                b'o' => {
                    if arg_idx < args.len() {
                        if let Ok(s) = core::str::from_utf8(args[arg_idx]) {
                            if let Ok(val) = parse_u64_dec(s) {
                                print_octal(val);
                            }
                        }
                        arg_idx += 1;
                    }
                    i += 2;
                }
                _ => {
                    // Unknown format specifier: print literally
                    let s = core::str::from_utf8(&fmt[i..i + 2]).unwrap_or("?");
                    stdout(s);
                    i += 2;
                }
            }
        } else {
            let c = [fmt[i]];
            if let Ok(s) = core::str::from_utf8(&c) {
                stdout(s);
            }
            i += 1;
        }
    }
}

fn parse_i64(s: &str) -> Result<i64, ()> {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return Err(());
    }
    let negative = bytes[0] == b'-';
    let start = if negative || bytes[0] == b'+' { 1 } else { 0 };
    let mut val: i64 = 0;
    for &b in &bytes[start..] {
        if !b.is_ascii_digit() {
            return Err(());
        }
        val = val.checked_mul(10).ok_or(())?;
        val = val.checked_add((b - b'0') as i64).ok_or(())?;
    }
    if negative {
        val = -val;
    }
    Ok(val)
}

fn parse_u64_dec(s: &str) -> Result<u64, ()> {
    let mut val: u64 = 0;
    for &b in s.as_bytes() {
        if !b.is_ascii_digit() {
            return Err(());
        }
        val = val.checked_mul(10).ok_or(())?;
        val = val.checked_add((b - b'0') as u64).ok_or(())?;
    }
    Ok(val)
}

fn parse_u64_hex(s: &str) -> Result<u64, ()> {
    let bytes = s.as_bytes();
    let start = if bytes.len() > 2 && bytes[0] == b'0' && (bytes[1] == b'x' || bytes[1] == b'X') {
        2
    } else {
        0
    };
    let mut val: u64 = 0;
    for &b in &bytes[start..] {
        val = val.checked_mul(16).ok_or(())?;
        match b {
            b'0'..=b'9' => val += (b - b'0') as u64,
            b'a'..=b'f' => val += (b - b'a' + 10) as u64,
            b'A'..=b'F' => val += (b - b'A' + 10) as u64,
            _ => return Err(()),
        }
    }
    Ok(val)
}

fn print_i64(val: i64) {
    if val < 0 {
        stdout("-");
        print_u64((-val) as u64);
    } else {
        print_u64(val as u64);
    }
}

fn print_u64(val: u64) {
    let mut buf = [0u8; 20];
    let s = common::format_u64(val, &mut buf);
    if let Ok(out) = core::str::from_utf8(s) {
        stdout(out);
    }
}

fn print_hex(val: u64) {
    if val == 0 {
        stdout("0");
        return;
    }
    let mut started = false;
    for i in 0..16 {
        let nibble = (val >> (60 - i * 4)) & 0xF;
        if nibble != 0 || started {
            started = true;
            let c = if nibble < 10 {
                b'0' + nibble as u8
            } else {
                b'a' + (nibble - 10) as u8
            };
            let s = [c];
            if let Ok(out) = core::str::from_utf8(&s) {
                stdout(out);
            }
        }
    }
}

fn print_octal(val: u64) {
    if val == 0 {
        stdout("0");
        return;
    }
    let mut started = false;
    // Max 22 octal digits for u64
    let mut i = 63;
    loop {
        if i < 3 {
            let digit = val & 0x7;
            let c = [b'0' + digit as u8];
            if let Ok(out) = core::str::from_utf8(&c) {
                stdout(out);
            }
            break;
        }
        let digit = (val >> i) & 0x7;
        if digit != 0 || started {
            started = true;
            let c = [b'0' + digit as u8];
            if let Ok(out) = core::str::from_utf8(&c) {
                stdout(out);
            }
        }
        i -= 3;
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
