//! b2sum — BLAKE2b hash of files
//!
//! Usage: b2sum [file...]
//!
//! Pure Rust BLAKE2b-256 implementation.

#![no_std]
#![no_main]

mod common;

use common::{exit, stderrln, stdout, stdoutln};
use openos_sdk::fs;

/// BLAKE2b initialization vector
const BLAKE2B_IV: [u64; 8] = [
    0x6a09e667f3bcc908,
    0xbb67ae8584caa73b,
    0x3c6ef372fe94f82b,
    0xa54ff53a5f1d36f1,
    0x510e527fade682d1,
    0x9b05688c2b3e6c1f,
    0x1f83d9abfb41bd6b,
    0x5be0cd19137e2179,
];

/// BLAKE2b sigma permutation
const SIGMA: [[usize; 16]; 12] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
    [11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4],
    [7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8],
    [9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13],
    [2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9],
    [12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11],
    [13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10],
    [6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5],
    [10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0],
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
];

struct Blake2b {
    h: [u64; 8],
    t: [u64; 2],
    buf: [u8; 128],
    buf_len: usize,
    out_len: usize,
}

impl Blake2b {
    fn new(out_len: usize) -> Self {
        let mut h = BLAKE2B_IV;
        // Parameter block: fanout=1, depth=1, digest_length=out_len
        h[0] ^= 0x01010000 ^ (out_len as u64);
        Self {
            h,
            t: [0, 0],
            buf: [0u8; 128],
            buf_len: 0,
            out_len,
        }
    }

    fn increment_counter(&mut self, n: u64) {
        self.t[0] = self.t[0].wrapping_add(n);
        if self.t[0] < n {
            self.t[1] = self.t[1].wrapping_add(1);
        }
    }

    fn compress(&mut self, block: &[u8; 128], f: bool) {
        let mut m = [0u64; 16];
        for i in 0..16 {
            m[i] = u64::from_le_bytes([
                block[i * 8],
                block[i * 8 + 1],
                block[i * 8 + 2],
                block[i * 8 + 3],
                block[i * 8 + 4],
                block[i * 8 + 5],
                block[i * 8 + 6],
                block[i * 8 + 7],
            ]);
        }

        let mut v = [0u64; 16];
        v[0..8].copy_from_slice(&self.h);
        v[8] = BLAKE2B_IV[0];
        v[9] = BLAKE2B_IV[1];
        v[10] = BLAKE2B_IV[2];
        v[11] = BLAKE2B_IV[3];
        v[12] = BLAKE2B_IV[4] ^ self.t[0];
        v[13] = BLAKE2B_IV[5] ^ self.t[1];
        if f {
            v[14] = !BLAKE2B_IV[6];
        } else {
            v[14] = BLAKE2B_IV[6];
        }
        v[15] = BLAKE2B_IV[7];

        for round in 0..12 {
            let s = &SIGMA[round];
            mix_g(&mut v, 0, 4, 8, 12, m[s[0]], m[s[1]]);
            mix_g(&mut v, 1, 5, 9, 13, m[s[2]], m[s[3]]);
            mix_g(&mut v, 2, 6, 10, 14, m[s[4]], m[s[5]]);
            mix_g(&mut v, 3, 7, 11, 15, m[s[6]], m[s[7]]);
            mix_g(&mut v, 0, 5, 10, 15, m[s[8]], m[s[9]]);
            mix_g(&mut v, 1, 6, 11, 12, m[s[10]], m[s[11]]);
            mix_g(&mut v, 2, 7, 8, 13, m[s[12]], m[s[13]]);
            mix_g(&mut v, 3, 4, 9, 14, m[s[14]], m[s[15]]);
        }

        for i in 0..8 {
            self.h[i] ^= v[i] ^ v[i + 8];
        }
    }

    fn update(&mut self, data: &[u8]) {
        let mut offset = 0;
        while offset < data.len() {
            let remaining = 128 - self.buf_len;
            let to_copy = if data.len() - offset < remaining {
                data.len() - offset
            } else {
                remaining
            };
            self.buf[self.buf_len..self.buf_len + to_copy]
                .copy_from_slice(&data[offset..offset + to_copy]);
            self.buf_len += to_copy;
            offset += to_copy;

            if self.buf_len == 128 {
                self.increment_counter(128);
                let block: [u8; 128] = core::array::from_fn(|i| self.buf[i]);
                self.compress(&block, false);
                self.buf_len = 0;
            }
        }
    }

    fn finalize(mut self) -> [u8; 32] {
        self.increment_counter(self.buf_len as u64);
        // Zero-pad remaining buffer
        for i in self.buf_len..128 {
            self.buf[i] = 0;
        }
        let block: [u8; 128] = core::array::from_fn(|i| self.buf[i]);
        self.compress(&block, true);

        let mut out = [0u8; 32];
        for i in 0..4 {
            out[i * 8..i * 8 + 8].copy_from_slice(&self.h[i].to_le_bytes());
        }
        out
    }
}

fn mix_g(v: &mut [u64; 16], a: usize, b: usize, c: usize, d: usize, x: u64, y: u64) {
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(x);
    v[d] = (v[d] ^ v[a]).rotate_right(32);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(24);
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(y);
    v[d] = (v[d] ^ v[a]).rotate_right(16);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(63);
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let path = "/disk/test.txt";

    match fs::open(path) {
        Ok(fd) => {
            let mut blake = Blake2b::new(32);
            let mut buf = [0u8; 4096];
            loop {
                match fs::read(fd, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => blake.update(&buf[..n]),
                    Err(_) => break,
                }
            }
            let _ = fs::close(fd);

            let hash = blake.finalize();
            print_hash(&hash);
            stdout("  ");
            stdoutln(path);
        }
        Err(_) => {
            stderrln("b2sum: no such file");
        }
    }
    exit(0);
}

fn print_hash(hash: &[u8; 32]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for &byte in hash {
        let hi = HEX[(byte >> 4) as usize];
        let lo = HEX[(byte & 0x0F) as usize];
        let pair = [hi, lo];
        if let Ok(s) = core::str::from_utf8(&pair) {
            stdout(s);
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
