//! base32 — encode or decode base32 data
//!
//! Usage: base32 [-d] [file]
//!
//! -d  decode mode (default is encode)

#![no_std]
#![no_main]

mod common;

use common::{exit, stderrln, stdout};
use openos_sdk::fs;

const B32_TABLE: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Demo: encode /disk/test.txt
    let decode_mode = false;
    let path = "/disk/test.txt";

    match fs::open(path) {
        Ok(fd) => {
            let mut buf = [0u8; 8192];
            match fs::read(fd, &mut buf) {
                Ok(n) => {
                    let data = &buf[..n];
                    if decode_mode {
                        base32_decode(data);
                    } else {
                        base32_encode(data);
                    }
                }
                Err(_) => {
                    stderrln("base32: read error");
                }
            }
            let _ = fs::close(fd);
        }
        Err(_) => {
            stderrln("base32: no such file");
        }
    }
    exit(0);
}

fn base32_encode(data: &[u8]) {
    let mut i = 0;
    let len = data.len();

    while i < len {
        // Collect 5 bytes (40 bits)
        let b0 = data[i] as u64;
        let b1 = if i + 1 < len { data[i + 1] as u64 } else { 0 };
        let b2 = if i + 2 < len { data[i + 2] as u64 } else { 0 };
        let b3 = if i + 3 < len { data[i + 3] as u64 } else { 0 };
        let b4 = if i + 4 < len { data[i + 4] as u64 } else { 0 };

        let quint = (b0 << 32) | (b1 << 24) | (b2 << 16) | (b3 << 8) | b4;

        // Extract 8 groups of 5 bits
        let num_chars = if i + 5 <= len {
            8
        } else {
            // Calculate how many base32 chars are needed
            let remaining = len - i;
            (remaining * 8 + 4) / 5
        };

        let mut out_buf = [0u8; 8];
        for j in 0..8 {
            let shift = 35 - j * 5;
            let idx = ((quint >> shift) & 0x1F) as usize;
            if j < num_chars {
                out_buf[j] = B32_TABLE[idx];
            } else {
                out_buf[j] = b'=';
            }
        }

        if let Ok(s) = core::str::from_utf8(&out_buf) {
            stdout(s);
        }
        i += 5;
    }
    stdout("\n");
}

fn base32_decode(data: &[u8]) {
    // Filter out whitespace and newlines
    let mut clean: [u8; 8192] = [0u8; 8192];
    let mut clean_len = 0;
    for &b in data {
        if !b.is_ascii_whitespace() && b != b'=' && clean_len < clean.len() {
            clean[clean_len] = b;
            clean_len += 1;
        }
    }

    let mut i = 0;
    while i + 8 <= clean_len {
        let mut val: u64 = 0;
        let mut valid_bits = 0u32;
        for j in 0..8 {
            if let Some(idx) = b32_decode_char(clean[i + j]) {
                val = (val << 5) | (idx as u64);
                valid_bits += 5;
            }
        }

        // Extract bytes from the 40-bit value
        let num_bytes = (valid_bits / 8) as usize;
        for j in 0..num_bytes {
            let shift = 32 - j * 8;
            let byte = ((val >> shift) & 0xFF) as u8;
            let out = [byte];
            if let Ok(s) = core::str::from_utf8(&out) {
                stdout(s);
            }
        }
        i += 8;
    }
    stdout("\n");
}

fn b32_decode_char(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a'),
        b'2'..=b'7' => Some(c - b'2' + 26),
        _ => None,
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
