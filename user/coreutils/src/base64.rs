//! base64 — encode or decode base64 data
//!
//! Usage: base64 [-d] [file]
//!
//! -d  decode mode (default is encode)

#![no_std]
#![no_main]

mod common;

use common::{exit, stderrln, stdout};
use openos_sdk::fs;

const B64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Demo: encode /disk/test.txt
    // In a real shell, -d and file come from argv.
    let decode_mode = false;
    let path = "/disk/test.txt";

    match fs::open(path) {
        Ok(fd) => {
            let mut buf = [0u8; 8192];
            match fs::read(fd, &mut buf) {
                Ok(n) => {
                    let data = &buf[..n];
                    if decode_mode {
                        base64_decode(data);
                    } else {
                        base64_encode(data);
                    }
                }
                Err(_) => {
                    stderrln("base64: read error");
                }
            }
            let _ = fs::close(fd);
        }
        Err(_) => {
            stderrln("base64: no such file");
        }
    }
    exit(0);
}

fn base64_encode(data: &[u8]) {
    let mut out_buf = [0u8; 4];
    let mut i = 0;
    let len = data.len();

    while i < len {
        let b0 = data[i] as u32;
        let b1 = if i + 1 < len { data[i + 1] as u32 } else { 0 };
        let b2 = if i + 2 < len { data[i + 2] as u32 } else { 0 };

        let triple = (b0 << 16) | (b1 << 8) | b2;

        out_buf[0] = B64_TABLE[((triple >> 18) & 0x3F) as usize];
        out_buf[1] = B64_TABLE[((triple >> 12) & 0x3F) as usize];

        if i + 1 < len {
            out_buf[2] = B64_TABLE[((triple >> 6) & 0x3F) as usize];
        } else {
            out_buf[2] = b'=';
        }

        if i + 2 < len {
            out_buf[3] = B64_TABLE[(triple & 0x3F) as usize];
        } else {
            out_buf[3] = b'=';
        }

        if let Ok(s) = core::str::from_utf8(&out_buf) {
            stdout(s);
        }
        i += 3;
    }
    stdout("\n");
}

fn base64_decode(data: &[u8]) {
    // Filter out whitespace and newlines
    let mut clean: [u8; 8192] = [0u8; 8192];
    let mut clean_len = 0;
    for &b in data {
        if !b.is_ascii_whitespace() && clean_len < clean.len() {
            clean[clean_len] = b;
            clean_len += 1;
        }
    }

    let mut out = [0u8; 3];
    let mut i = 0;
    while i < clean_len {
        let c0 = b64_decode_char(clean[i]);
        let c1 = if i + 1 < clean_len {
            b64_decode_char(clean[i + 1])
        } else {
            0
        };
        let c2 = if i + 2 < clean_len && clean[i + 2] != b'=' {
            b64_decode_char(clean[i + 2])
        } else {
            0xFF
        };
        let c3 = if i + 3 < clean_len && clean[i + 3] != b'=' {
            b64_decode_char(clean[i + 3])
        } else {
            0xFF
        };

        if c0 == 0xFF || c1 == 0xFF {
            break;
        }

        let triple = ((c0 as u32) << 18) | ((c1 as u32) << 12);
        out[0] = ((triple >> 16) & 0xFF) as u8;
        stdout_bytes(&out[..1]);

        if c2 != 0xFF {
            let triple = triple | ((c2 as u32) << 6);
            out[1] = ((triple >> 8) & 0xFF) as u8;
            stdout_bytes(&out[1..2]);
        }

        if c3 != 0xFF {
            let triple = triple | (c3 as u32);
            out[2] = (triple & 0xFF) as u8;
            stdout_bytes(&out[2..3]);
        }

        i += 4;
    }
    stdout("\n");
}

fn stdout_bytes(data: &[u8]) {
    if let Ok(s) = core::str::from_utf8(data) {
        let _ = openos_sdk::console::write(s);
    }
}

fn b64_decode_char(c: u8) -> u32 {
    match c {
        b'A'..=b'Z' => (c - b'A') as u32,
        b'a'..=b'z' => (c - b'a' + 26) as u32,
        b'0'..=b'9' => (c - b'0' + 52) as u32,
        b'+' => 62,
        b'/' => 63,
        _ => 0xFF,
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
