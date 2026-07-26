//! cksum — CRC-32 checksum of files
//!
//! Usage: cksum [file...]
//!
//! Computes CRC-32 (ISO 3309 / ITU-T V.42) checksum.

#![no_std]
#![no_main]

mod common;

use common::{exit, stderrln, stdout, stdoutln};
use openos_sdk::fs;

/// CRC-32 lookup table (polynomial 0xEDB88320, reflected)
const CRC32_TABLE: [u32; 256] = build_crc32_table();

const fn build_crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0u32;
    while i < 256 {
        let mut crc = i;
        let mut j = 0;
        while j < 8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB88320;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i as usize] = crc;
        i += 1;
    }
    table
}

fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    for &byte in data {
        let idx = ((crc ^ byte as u32) & 0xFF) as usize;
        crc = (crc >> 8) ^ CRC32_TABLE[idx];
    }
    crc
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let path = "/disk/test.txt";

    match fs::open(path) {
        Ok(fd) => {
            let mut crc: u32 = 0xFFFFFFFF;
            let mut total_len: u64 = 0;
            let mut buf = [0u8; 4096];

            loop {
                match fs::read(fd, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        crc = crc32_update(crc, &buf[..n]);
                        total_len += n as u64;
                    }
                    Err(_) => break,
                }
            }
            let _ = fs::close(fd);

            crc ^= 0xFFFFFFFF;

            print_u32(crc);
            stdout(" ");
            print_u64(total_len);
            stdout(" ");
            stdoutln(path);
        }
        Err(_) => {
            stderrln("cksum: no such file");
        }
    }
    exit(0);
}

fn print_u32(mut val: u32) {
    if val == 0 {
        stdout("0");
        return;
    }
    let mut buf = [0u8; 10];
    let mut pos = 10;
    while val > 0 {
        pos -= 1;
        buf[pos] = b'0' + (val % 10) as u8;
        val /= 10;
    }
    if let Ok(s) = core::str::from_utf8(&buf[pos..]) {
        stdout(s);
    }
}

fn print_u64(mut val: u64) {
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
